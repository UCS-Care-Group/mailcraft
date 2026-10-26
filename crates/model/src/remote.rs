// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar events and contacts that an account's server keeps besides its mail.
//!
//! Plain data, like [`crate::sync`]: a host with sockets (`mailcraft-net`, which asks Microsoft
//! Graph) brings an [`EventsDownload`] or a [`ContactsDownload`], and the engine merges it into
//! the mailbox (`sync.applyEvents`, `sync.applyContacts`). What came from a server says so in its
//! [`Origin`], which is also what a change sent back to the server would name the item by.
//!
//! All of it is hostile: it comes from a server, or from a command's parameters.
//! [`RemoteEvent::tidy`] and [`RemoteContact::tidy`] cut it to size before any of it is kept.

use mailcraft_time::{Date, DateTime, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{
    Account, Address, Attendee, CalendarOwner, Category, Event, FolderKind, Id, Importance, Labeled, MAX_NAME, OnlineMeeting, OnlineProvider,
    PostalAddress, Recurrence, Response, ResponseOptions, Rgb, Rights, ShowAs,
};

/// Most events and most contacts one download is taken to hold; what is beyond is left out.
pub const MAX_EVENTS: usize = 20_000;
pub const MAX_CONTACTS: usize = 20_000;
/// Longest id or change key a server may give an item. A longer one is no id at all: cut short,
/// two items could end up with the same.
pub const MAX_KEY: usize = 1_024;
/// Longest title, place or name, and longest description or note.
const MAX_LINE: usize = 500;
const MAX_TEXT: usize = 100_000;
/// Most people in one event, and most addresses, numbers and the like of one contact.
const MAX_ATTENDEES: usize = 500;
const MAX_ENTRIES: usize = 50;
/// The longest a reminder is ahead of its event: four weeks, as for an event made here.
const MAX_REMINDER: u32 = 40_320;

/// Where an event, a contact or a calendar comes from when an account's server keeps it. The
/// item here is then a copy of the server's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Origin {
    /// The account whose server keeps the item; 0 for an item that lives in this mailbox only.
    pub account: Id,
    /// What the server calls the item. Empty for a calendar that is the account's main one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The server's mark of the version that was downloaded (a change key or an ETag): what a
    /// change sent back would show, to be refused if the server's copy has changed since.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    /// For one occurrence of a series: what the server calls the series.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub series: String,
    /// For an event: somebody else organizes it, and the account's owner is invited. What is
    /// changed here is then changed in the owner's own copy only.
    #[serde(skip_serializing_if = "is_false")]
    pub guest: bool,
    /// For one occurrence of a series that was changed on its own (or that doesn't fall where
    /// the series' rule puts it here): the day of the series it stands in for, as the series has
    /// it here. The series leaves that day out, and this event is what happens on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<Date>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Origin {
    /// The item lives in this mailbox only.
    pub fn is_local(&self) -> bool {
        self.account == 0
    }

    /// An account's server keeps the item.
    pub fn is_remote(&self) -> bool {
        !self.is_local()
    }
}

/// What a server's event is to a series.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteEventKind {
    /// An event of its own.
    #[default]
    Single,
    /// One occurrence of a series, as the series has it.
    Occurrence,
    /// One occurrence that was changed on its own.
    Exception,
    /// The series itself. With its rule ([`RemoteEvent::recurrence`]) it is one event here, which
    /// repeats; its occurrences in the stretch come with it, and say which days it really falls
    /// on. Without a rule (one that can't be said here) it is passed over, and its occurrences
    /// are events of their own.
    SeriesMaster,
}

/// An event as a server has it. A series comes as itself and as its occurrences in the stretch
/// that was asked for, one event each.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteEvent {
    /// What the server calls it. An event without one is not merged: nothing would tell the
    /// next download that it is here already.
    pub id: String,
    /// The server's mark of this version of it ([`Origin::change_key`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    /// Its iCalendar `UID`, the same in every calendar the event is in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uid: String,
    pub title: String,
    pub location: String,
    /// Plain text.
    pub description: String,
    /// When it starts and ends (Unix seconds, UTC). For a whole-day event: midnight (UTC) of its
    /// first day, and of the day after its last, as the server names the days.
    pub start: Timestamp,
    pub end: Timestamp,
    pub all_day: bool,
    /// The time zones the event was made in, as the server names them (`Pacific Standard Time`,
    /// `Europe/Berlin`, `UTC`): for its start and for its end; for a series, the zone its rule
    /// counts by. Empty when the server didn't say. The times above are instants either way;
    /// these say where the event's own clock is ([`Event::start_zone`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start_zone: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub end_zone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,
    /// What the account's owner answered, when the event is somebody else's meeting.
    pub response: Response,
    pub show_as: ShowAs,
    pub private: bool,
    pub cancelled: bool,
    pub kind: RemoteEventKind,
    /// For an occurrence or an exception: what the server calls the series.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub series: String,
    /// Remind this many minutes before the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_minutes: Option<u32>,
    /// When it was last changed on the server (Unix seconds); 0 when the server didn't say.
    pub modified: Timestamp,
    /// Whether the account's owner organizes it, when the server said; `None` leaves it to be
    /// told from the organizer's address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_organizer: Option<bool>,
    /// The mark it was made with, for an event that was made here ([`EventChange::token`]): it
    /// is how a new event is known again when the server's answer to making it was lost.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// For a series: how it repeats, when that can be said as the mailbox says it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,
    /// For a series: the day its rule counts from, in the zone the series was made in. It is the
    /// day of the first occurrence there, which may be the day before or after it here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_day: Option<Date>,
    /// For an occurrence or an exception: when the series has it (Unix seconds), whenever it
    /// happens now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<Timestamp>,
    /// Its categories, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// How much it matters. (This and the four below are what
    /// [`EventsDownload::has_options`] says a download has.)
    pub importance: Importance,
    /// Whether the people invited are asked to answer, and may propose another time.
    pub options: ResponseOptions,
    /// It is held online: with what, and the link to join it by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<OnlineMeeting>,
    /// The other times that people invited have proposed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<RemoteProposal>,
    /// The server keeps files with it.
    #[serde(skip_serializing_if = "is_false")]
    pub attachments: bool,
}

/// Another time that somebody invited proposes for a meeting, as a server has it (Unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteProposal {
    pub from: Address,
    pub start: Timestamp,
    pub end: Timestamp,
}

impl RemoteEvent {
    /// Cut everything to the size the mailbox keeps: ids that can't be ids are emptied, titles
    /// are one line, text loses its control characters, lists are capped, and the end is not
    /// before the start. Doing it twice changes nothing more.
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
        self.change_key = key(&self.change_key);
        self.series = key(&self.series);
        self.token = mark(&self.token);
        self.uid = line(&self.uid, MAX_KEY);
        self.title = line(&self.title, MAX_LINE);
        self.location = line(&self.location, MAX_LINE);
        self.description = text(&self.description, MAX_TEXT);
        self.organizer = self.organizer.take().and_then(person);
        let attendees = std::mem::take(&mut self.attendees);
        self.attendees = attendees
            .into_iter()
            .filter_map(|attendee| person(attendee.address).map(|address| Attendee { address, ..attendee }))
            .take(MAX_ATTENDEES)
            .collect();
        self.reminder_minutes = self.reminder_minutes.map(|minutes| minutes.min(MAX_REMINDER));
        self.end = self.end.max(self.start);
        self.start_zone = line(&self.start_zone, crate::zoned::MAX_ZONE);
        self.end_zone = line(&self.end_zone, crate::zoned::MAX_ZONE);
        self.recurrence = self.recurrence.take().map(tidy_rule);
        self.series_day = self.series_day.and_then(|day| Date::new(day.year, day.month, day.day));
        self.categories = names(std::mem::take(&mut self.categories));
        // A link to join by is an `https` address or it is none.
        self.online = self.online.take().map(OnlineMeeting::tidied);
        self.proposals = std::mem::take(&mut self.proposals)
            .into_iter()
            .filter_map(|proposal| person(proposal.from).map(|from| RemoteProposal { from, end: proposal.end.max(proposal.start), ..proposal }))
            .take(MAX_ATTENDEES)
            .collect();
    }
}

/// A rule for repeating from outside, within what the mailbox counts with.
fn tidy_rule(mut rule: Recurrence) -> Recurrence {
    rule.interval = rule.interval.clamp(1, 999);
    rule.by_day.sort();
    rule.by_day.dedup();
    rule.nth = rule.nth.filter(|nth| matches!(nth, 1..=5 | -1));
    rule.count = rule.count.filter(|count| *count > 0).map(|count| count.min(9_999));
    rule.until = rule.until.and_then(|day| Date::new(day.year, day.month, day.day));
    rule
}

/// Category names from outside: one line each, none empty, none twice, a few at most.
fn names(list: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in list.iter().map(|name| line(name, MAX_NAME)).filter(|name| !name.is_empty()) {
        if out.len() < MAX_ENTRIES && !out.iter().any(|have| have.eq_ignore_ascii_case(&name)) {
            out.push(name);
        }
    }
    out
}

/// A contact as a server has it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteContact {
    /// What the server calls it; a contact without one is not merged.
    pub id: String,
    /// The server's mark of this version of it ([`Origin::change_key`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    pub display_name: String,
    pub first: String,
    pub middle: String,
    pub last: String,
    pub nickname: String,
    pub company: String,
    /// The job title.
    pub title: String,
    pub department: String,
    pub emails: Vec<Labeled>,
    pub phones: Vec<Labeled>,
    pub addresses: Vec<PostalAddress>,
    pub websites: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<Date>,
    pub notes: String,
    /// When it was last changed on the server (Unix seconds); 0 when the server didn't say.
    pub modified: Timestamp,
}

impl RemoteContact {
    /// Cut everything to the size the mailbox keeps, as [`RemoteEvent::tidy`] does: names are
    /// one line, an e-mail address is one or it is left out, empty entries go, lists are capped.
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
        self.change_key = key(&self.change_key);
        for name in [
            &mut self.display_name,
            &mut self.first,
            &mut self.middle,
            &mut self.last,
            &mut self.nickname,
            &mut self.company,
            &mut self.title,
            &mut self.department,
        ] {
            *name = line(name, MAX_LINE);
        }
        self.notes = text(&self.notes, MAX_TEXT);
        // A day that no calendar has is no birthday.
        self.birthday = self.birthday.and_then(|day| Date::new(day.year, day.month, day.day));
        // Entries with nothing in them go before the count is taken: they are how a server says
        // "no mobile number".
        let labeled = |list: &mut Vec<Labeled>, value: &dyn Fn(&str) -> String| {
            *list = std::mem::take(list)
                .into_iter()
                .map(|entry| Labeled { label: line(&entry.label, MAX_LINE), value: value(&entry.value) })
                .filter(|entry| !entry.value.is_empty())
                .take(MAX_ENTRIES)
                .collect();
        };
        labeled(&mut self.emails, &mailbox);
        labeled(&mut self.phones, &|number: &str| line(number, MAX_LINE));
        self.addresses = std::mem::take(&mut self.addresses)
            .into_iter()
            .map(|a| PostalAddress {
                label: line(&a.label, MAX_LINE),
                street: text(&a.street, MAX_LINE),
                city: line(&a.city, MAX_LINE),
                region: line(&a.region, MAX_LINE),
                postal_code: line(&a.postal_code, MAX_LINE),
                country: line(&a.country, MAX_LINE),
            })
            .filter(|a| !a.is_empty())
            .take(MAX_ENTRIES)
            .collect();
        self.websites = std::mem::take(&mut self.websites)
            .into_iter()
            .map(|site| line(&site, MAX_LINE))
            .filter(|site| !site.is_empty())
            .take(MAX_ENTRIES)
            .collect();
    }
}

/// The events of a stretch of one calendar, as its server has them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventsDownload {
    /// The account whose calendar it is. Whoever asked the server fills it in
    /// ([`EventsDownload::for_account`]): the server only knows who signed in.
    pub account: Id,
    /// That account's address: a download is not taken in by another account that happens to
    /// have the id (in a mailbox opened while the download was under way).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    /// What the server calls the calendar; empty for the account's main one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub calendar: String,
    /// The stretch that was asked for (Unix seconds, UTC): from `from` up to, not including, `to`.
    pub from: Timestamp,
    pub to: Timestamp,
    /// Everything the server has in the stretch is here. When it is not (the server had more
    /// than is taken, or said something that couldn't be read), what is here is still merged, but
    /// nothing is removed from the mailbox for not being here.
    pub complete: bool,
    pub events: Vec<RemoteEvent>,
    /// Series the server was asked for by name and no longer has: they go here too.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gone: Vec<String>,
    /// The events say what categories they have. When not (a server that couldn't be asked for
    /// them), an event's categories stay as they are here.
    #[serde(skip_serializing_if = "is_false")]
    pub has_categories: bool,
    /// The events say how much they matter, what the people invited are asked for, whether they
    /// are held online, who proposed another time and whether files are kept with them. When
    /// not (a server that couldn't be asked for that), all of it stays as it is here.
    #[serde(skip_serializing_if = "is_false")]
    pub has_options: bool,
    /// The calendar can't be listed by this account, only its free and busy times can be had
    /// (#70): the events are blocks of time without a title or a place, and the calendar is
    /// shown as such ([`crate::Rights::FreeBusy`]).
    #[serde(skip_serializing_if = "is_false")]
    pub free_busy: bool,
}

impl EventsDownload {
    /// The download as one for `account`.
    pub fn for_account(mut self, account: &Account) -> EventsDownload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

/// An account's contacts, as its server has them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactsDownload {
    /// The account whose contacts they are ([`ContactsDownload::for_account`]).
    pub account: Id,
    /// That account's address, as in [`EventsDownload::email`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    /// Every contact the server has is here. When not, nothing is removed for not being here.
    pub complete: bool,
    pub contacts: Vec<RemoteContact>,
}

impl ContactsDownload {
    /// The download as one for `account`.
    pub fn for_account(mut self, account: &Account) -> ContactsDownload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

/// Most changes to events that may wait for a server at once ([`crate::Mailbox::event_changes`]),
/// and most that one [`EventsUpload`] carries: the rest go with the next.
pub const MAX_EVENT_CHANGES: usize = 5_000;
pub const MAX_EVENT_OPS: usize = 50;
/// Longest mark a new event is sent with ([`EventChange::token`]).
pub const MAX_TOKEN: usize = 64;
/// Longest text kept of why a server didn't take a change.
const MAX_WHY: usize = 300;
/// Longest note that goes along with a cancellation or an answer.
pub const MAX_COMMENT: usize = 2_000;

/// What is done to an event that a server keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventChangeKind {
    /// An event made here that the server doesn't have yet.
    #[default]
    Create,
    /// An event the server has that was changed here.
    Update,
    /// An event the server has that was deleted here.
    Delete,
    /// Somebody else's meeting that the owner answered here ([`EventChange::response`]).
    Respond,
    /// A meeting the server has that the owner sends on to more people ([`EventChange::to`]).
    /// Nothing changes here; the server writes to them.
    Forward,
}

/// Most people one meeting is forwarded to at once.
pub const MAX_FORWARD: usize = 100;

/// A change made here to an event of a calendar that an account's server keeps, which the server
/// is yet to be told. The change itself is already made in the mailbox; this is the note that it
/// has to go to the server too. A host with sockets pushes it ([`EventsUpload`]) and it stays
/// listed until the server has taken it on, or has refused it for good ([`EventsUploaded`]).
///
/// What exactly is sent is worked out when it is sent, from the event as it is then: a new event
/// goes whole, a changed one as what differs from `base`. So several changes to one event before
/// the server hears of any are one change, and one that is taken back is none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventChange {
    pub account: Id,
    pub kind: EventChangeKind,
    /// The event in the mailbox; for a deletion, the id it had.
    pub event: Id,
    /// What the server calls the event. Empty for one it doesn't have yet.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// For a new event: the mark it is sent with. Sending it a second time (the answer to the
    /// first was lost) then makes no second event, and the event is known again when a download
    /// brings it before the answer.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// For a change or a deletion: the event as the server has it. What is sent is what differs
    /// from this, and this is what is put back when the server refuses for good.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Box<Event>>,
    /// How many times in a row the server couldn't be told.
    #[serde(skip_serializing_if = "is_zero")]
    pub failures: u32,
    /// Not to be tried again before this (Unix seconds); 0 for at once.
    #[serde(skip_serializing_if = "is_zero_time")]
    pub retry_at: Timestamp,
    /// The server has said once that the account may not do this. It is tried once more, after
    /// the owner has been asked for the permission, and given up when it is refused again.
    #[serde(skip_serializing_if = "is_false")]
    pub denied: bool,
    /// Why it didn't go through the last time, for the owner to read.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// For a change to one occurrence of a series that the server has no name of its own for
    /// here (`remote` is then empty): what the server calls the series, and when the series has
    /// that occurrence (Unix seconds). Whoever sends the change asks the server for the
    /// occurrence first.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub series: String,
    #[serde(skip_serializing_if = "is_zero_time")]
    pub occurrence: Timestamp,
    /// The owner said yes to the mail this makes the server send: invitations to a new meeting,
    /// an update or a cancellation to the people invited, an answer to the organizer. A change
    /// that would make the server write to anybody is not sent without it.
    #[serde(skip_serializing_if = "is_false")]
    pub send: bool,
    /// What the owner wrote to go along with a cancellation or an answer.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// For an answer: what it is.
    #[serde(skip_serializing_if = "is_no_response")]
    pub response: Response,
    /// For an answer (tentative, or declined): another time the owner proposes to the organizer
    /// with it. It is mail to the organizer: only with [`EventChange::send`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<EventTime>,
    /// For a meeting that is forwarded: who to. It is mail to them: only with
    /// [`EventChange::send`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<Address>,
    /// For a new event that is an appointment moved here from another of the account's
    /// calendars (a server has no way of moving one): what the server calls the original, which
    /// it is to delete once it has made this one. `base` is then the original, to be put back
    /// should either step be refused.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub replaces: String,
    /// For the deletion that finishes such a move: what the server calls the copy it made,
    /// which goes again should the server refuse to delete the original.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub moved: String,
    /// For that deletion when the copy was made in a calendar of another account (or of another
    /// mailbox's, #70): the account that has the copy, which is not this change's. 0 for a move
    /// within one account.
    #[serde(skip_serializing_if = "is_no_id")]
    pub copy_account: Id,
}

fn is_no_id(id: &Id) -> bool {
    *id == 0
}

fn is_no_response(response: &Response) -> bool {
    *response == Response::None
}

fn is_no_status(status: &u16) -> bool {
    *status == 0
}

/// A short tag for what a server calls an event, for a log line: two lines about the same event
/// carry the same tag, and nothing of the id, let alone of the event, can be read from it.
pub fn id_tag(id: &str) -> String {
    // (FNV-1a, 32 bits: a name for a line of a log, not a secret.)
    let mut hash: u32 = 0x811C_9DC5;
    for byte in id.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

impl EventChange {
    /// It is about one occurrence of a series that is known by its series and its time only.
    pub fn of_occurrence(&self) -> bool {
        self.remote.is_empty() && !self.series.is_empty() && self.kind != EventChangeKind::Create
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_zero_time(t: &Timestamp) -> bool {
    *t == 0
}

/// When an event is, as a server counts it: instants (Unix seconds, UTC); for a whole-day event
/// the midnight (UTC) of its first day and of the day after its last, as in [`RemoteEvent`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventTime {
    pub start: Timestamp,
    pub end: Timestamp,
    pub all_day: bool,
}

/// Whether an event reminds, and how many minutes before it starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct EventReminder {
    pub on: bool,
    pub minutes: u32,
}

/// What a server is told of an event: everything for a new one, and for one it has only what was
/// changed here (`None` leaves a field as the server has it: somebody else may have changed it
/// meanwhile, and the server knows more of an event than is kept here).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Plain text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The start, the end and whether it is a whole-day event: together, or not at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<EventTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_as: Option<ShowAs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder: Option<EventReminder>,
    /// For a series, with `time`: the start and the end of its first occurrence by the clock
    /// it repeats by: the clock of its own zone when it has one (`zones`), else the clock here.
    /// A series repeats by the clock (nine is nine on both sides of a daylight-saving
    /// change), which instants can't say: the server is told the clock times and the zone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall: Option<EventWall>,
    /// With `time`, for a timed event that has zones of its own ([`Event::start_zone`]): their
    /// names, for the server to keep the event in. `None` for an event without: it is told as
    /// it always was (instants, and for a series the clock here).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zones: Option<EventZones>,
    /// How it repeats: for a new series, and for one whose rule or first day changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<EventRepeat>,
    /// Who is invited (the address and whether optional; what they answered is the server's to
    /// say). Telling a server this makes it write to them: [`EventChange::send`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attendees: Option<Vec<Attendee>>,
    /// Its categories, by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// How much it matters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importance: Option<Importance>,
    /// Whether the people invited are asked to answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_responses: Option<bool>,
    /// Whether they may propose another time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_proposals: Option<bool>,
    /// It is to be held online, with this. (A server that has made a meeting online keeps it so:
    /// there is no telling it otherwise, so nothing here says "not online".)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<OnlineProvider>,
}

/// The start and the end of a series' first occurrence by the clock here ([`EventFields::wall`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct EventWall {
    pub start: DateTime,
    pub end: DateTime,
}

/// The zones a timed event has of its own, by name ([`EventFields::zones`]): the names of the
/// IANA database, `UTC`, or an offset (`UTC+05:30`), as [`Event::start_zone`] has them. The end's
/// is the start's when the event names none for it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct EventZones {
    pub start: String,
    pub end: String,
}

/// How an event repeats, as a server is told ([`EventFields::repeat`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventRepeat {
    /// The rule; `None` for an event that no longer repeats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<Recurrence>,
    /// The day of the first occurrence here, which the rule counts from.
    pub first: Date,
}

impl EventFields {
    /// Nothing is said: there is nothing to tell the server.
    pub fn is_empty(&self) -> bool {
        *self == EventFields::default()
    }

    /// Which of them are said, by the names a server has for them: for a log line, which may
    /// say what kind of thing was sent and never what it was.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        let mut said = |is: bool, name: &'static str| {
            if is {
                names.push(name);
            }
        };
        said(self.title.is_some(), "subject");
        said(self.location.is_some(), "location");
        said(self.description.is_some(), "body");
        said(self.time.is_some(), "start");
        said(self.time.is_some(), "end");
        said(self.time.is_some(), "isAllDay");
        said(self.show_as.is_some(), "showAs");
        said(self.private.is_some(), "sensitivity");
        said(self.reminder.is_some(), "reminder");
        said(self.repeat.is_some(), "recurrence");
        said(self.attendees.is_some(), "attendees");
        said(self.categories.is_some(), "categories");
        said(self.importance.is_some(), "importance");
        said(self.request_responses.is_some(), "responseRequested");
        said(self.allow_proposals.is_some(), "allowNewTimeProposals");
        said(self.online.is_some(), "isOnlineMeeting");
        names
    }

    /// Cut to the size a server is sent: one-line titles and places, text without control
    /// characters, an end that is not before the start, a reminder of four weeks at most.
    pub fn tidy(&mut self) {
        self.title = self.title.take().map(|title| line(&title, MAX_LINE));
        self.location = self.location.take().map(|place| line(&place, MAX_LINE));
        self.description = self.description.take().map(|words| text(&words, MAX_TEXT));
        if let Some(time) = &mut self.time {
            time.end = time.end.max(time.start);
        }
        if let Some(reminder) = &mut self.reminder {
            reminder.minutes = reminder.minutes.min(MAX_REMINDER);
        }
        if let Some(wall) = &mut self.wall {
            wall.end = wall.end.max(wall.start);
        }
        // (A name that is no zone stays a name: the times beside it were worked out for it, and
        // whoever sends them has to see that it can't be sent.)
        self.zones = self
            .zones
            .take()
            .map(|zones| EventZones { start: line(&zones.start, crate::zoned::MAX_ZONE), end: line(&zones.end, crate::zoned::MAX_ZONE) });
        self.zones = self.zones.take().filter(|zones| !zones.start.is_empty());
        if let Some(repeat) = &mut self.repeat {
            repeat.rule = repeat.rule.take().map(tidy_rule);
        }
        // Somebody without an address can't be invited; nobody is invited twice.
        self.attendees = self.attendees.take().map(|list| {
            let mut invited: Vec<Attendee> = Vec::new();
            for attendee in list {
                let Some(address) = person(attendee.address).filter(|address| !address.email.is_empty()) else { continue };
                if invited.len() < MAX_ATTENDEES && !invited.iter().any(|have| have.address.email.eq_ignore_ascii_case(&address.email)) {
                    invited.push(Attendee { address, optional: attendee.optional, response: Response::None });
                }
            }
            invited
        });
        self.categories = self.categories.take().map(names);
    }
}

/// One thing a server is asked to do to an event ([`EventsUpload`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventOp {
    pub kind: EventChangeKind,
    /// The event in the mailbox the change is about (for the answer to find it again).
    pub event: Id,
    /// What the server calls the event, for a change or a deletion.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// The mark a new event is made with ([`EventChange::token`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// A new event whole, or what changed of one the server has; nothing for a deletion.
    pub fields: EventFields,
    /// For one occurrence of a series that has no name of its own here ([`EventChange::series`]):
    /// the series, and when the series has the occurrence. The occurrence is asked for first.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub series: String,
    #[serde(skip_serializing_if = "is_zero_time")]
    pub occurrence: Timestamp,
    /// The server's calendar the event is in (for a new event: the one it goes into); empty for
    /// the account's main one. An event of another calendar is asked for through that calendar:
    /// one that somebody else shares with the account is not reached any other way.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub calendar: String,
    /// For a deletion: the owner organizes the meeting and people are invited, so it is called
    /// off, with `comment` for them to read, rather than just deleted.
    #[serde(skip_serializing_if = "is_false")]
    pub cancel: bool,
    /// For an answer: what it is, and whether the organizer is told (with `comment`).
    #[serde(skip_serializing_if = "is_no_response")]
    pub response: Response,
    #[serde(skip_serializing_if = "is_false")]
    pub send: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// For an answer that is sent: another time the owner proposes with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<EventTime>,
    /// For a meeting that is forwarded: who to (with `comment` for them to read).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<Address>,
}

impl EventOp {
    /// Cut to size, as everything from outside is: an id that can't be one is none.
    pub fn tidy(&mut self) {
        self.remote = key(&self.remote);
        self.token = mark(&self.token);
        self.series = key(&self.series);
        self.calendar = key(&self.calendar);
        self.comment = text(&self.comment, MAX_COMMENT);
        self.fields.tidy();
        if let Some(time) = &mut self.proposed {
            time.end = time.end.max(time.start);
        }
        self.to = recipients(std::mem::take(&mut self.to));
    }

    /// Whether the server sends mail to other people for this change (#70): an answer that is
    /// sent, a meeting called off, a meeting forwarded, and a meeting made or changed with
    /// people invited. From a shared mailbox that is sending from it.
    pub fn mails_others(&self) -> bool {
        match self.kind {
            EventChangeKind::Respond | EventChangeKind::Forward => self.send,
            EventChangeKind::Delete => self.cancel,
            EventChangeKind::Create | EventChangeKind::Update => self.fields.attendees.as_ref().is_some_and(|who| !who.is_empty()),
        }
    }

    /// It is about one occurrence of a series, known by its series and its time only.
    pub fn of_occurrence(&self) -> bool {
        self.remote.is_empty() && !self.series.is_empty() && self.kind != EventChangeKind::Create
    }
}

/// The changes to the events of an account's main calendar that its server is to be told, oldest
/// first. The engine says what they are (`sync.eventChanges`); a host with sockets carries them
/// out, one after the other, and brings back an [`EventsUploaded`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventsUpload {
    pub account: Id,
    /// That account's address, as in [`EventsDownload::email`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub ops: Vec<EventOp>,
    /// The zone the calendar is shown in, by its name (`Europe/Berlin`): the zone the days of a
    /// whole-day event are days in, and the zone whose clock a series without a zone of its own
    /// is told by ([`EventFields::wall`]). Empty when it has no name; whoever carries the upload
    /// out then uses what it knows of this computer's zone.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zone: String,
}

/// What a server made of one [`EventOp`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventResult {
    /// It was done.
    Done,
    /// The server no longer has the event: a deletion is done, a change has nothing to change.
    Gone,
    /// The server won't have it, and asking again would not help.
    Refused,
    /// The account may not do this: the owner hasn't given the permission, or it can't be had.
    Denied,
    /// It couldn't be done now (no connection, a busy server): to be tried again later.
    #[default]
    Failed,
}

/// What became of one [`EventOp`], with the op itself: the mailbox may have moved on since it was
/// asked for, and what the server was told is how that is noticed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventOutcome {
    pub op: EventOp,
    pub result: EventResult,
    /// What the server calls the event now (for one that was made) and its mark of the version
    /// it has now ([`Origin::change_key`]), when it said.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    /// The iCalendar `UID` the server gave a new event.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uid: String,
    /// When the server says the event was last changed, after this (Unix seconds); 0 when the
    /// answer didn't say.
    #[serde(skip_serializing_if = "is_zero_time")]
    pub modified: Timestamp,
    /// The status of the server's answer; 0 when there was none (it wasn't asked, or couldn't be
    /// reached).
    #[serde(skip_serializing_if = "is_no_status")]
    pub status: u16,
    /// The time zone the event's times were said in, when any were sent (`UTC` for instants).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zone: String,
    /// Why not, in words for the owner; never anything that was sent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// How many seconds the server asked to be left alone, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u32>,
    /// For an event that was to be held online ([`EventFields::online`]): whether the server
    /// made it so, when it said. `Some(false)`: the event was made (or changed) without the
    /// online meeting, because the server wouldn't have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    /// The link to join the meeting by, when the server's answer has it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub join_url: String,
    /// For a change that said whether another time may be proposed
    /// ([`EventFields::allow_proposals`]): `Some(false)` when the server wouldn't have that
    /// changed, and took the rest of the change without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_times: Option<bool>,
}

impl EventOutcome {
    /// Cut to size, as everything from outside is.
    pub fn tidy(&mut self) {
        self.op.tidy();
        self.remote = key(&self.remote);
        self.change_key = key(&self.change_key);
        self.uid = line(&self.uid, MAX_KEY);
        self.error = line(&self.error, MAX_WHY);
        self.join_url = OnlineMeeting::clean_link(&self.join_url).unwrap_or_default();
        self.zone = line(&self.zone, 64);
    }
}

/// What a server made of an [`EventsUpload`]: an outcome for every op that was tried, in order.
/// An op without one was not tried (the server stopped taking the sign-in, or was too busy), and
/// waits as before.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventsUploaded {
    pub account: Id,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub outcomes: Vec<EventOutcome>,
    /// The server stopped taking the sign-in part-way: a new one is needed for the rest.
    #[serde(skip_serializing_if = "is_false")]
    pub token_refused: bool,
}

impl EventsUpload {
    /// The upload as one for `account`.
    pub fn for_account(mut self, account: &Account) -> EventsUpload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

/// Most changes that wait to be seen on the server at once ([`crate::Mailbox::event_checks`]).
pub const MAX_EVENT_CHECKS: usize = 1_000;
/// How many downloads may show an event otherwise than it was sent before the mailbox stops
/// taking the server's word that it took the change: a server may answer a question put right
/// after a change with what it had before.
pub const MAX_CHECK_TRIES: u32 = 3;
/// How long a change waits to be seen on the server when no download covers its event (seconds).
pub const MAX_CHECK_AGE: i64 = 86_400;

/// A change that a server said it took, which the mailbox is yet to see for itself: the next
/// download of the event's calendar has to show the event as it was sent (or gone, for one that
/// was deleted). Until it does, or [`MAX_CHECK_TRIES`] downloads didn't, a download leaves the
/// event as it is here; after that the event is put back to what the server has, and the owner
/// is told. A "yes" from a server is not taken for the change having arrived.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventCheck {
    pub account: Id,
    /// What was done: an event made or changed is to be there as it was sent, one that was
    /// deleted is to be gone.
    pub kind: EventChangeKind,
    /// The event in the mailbox; for a deletion, the id it had.
    pub event: Id,
    /// What the server calls the event.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// What was sent of it that a download can show again as it was sent: the times, the title,
    /// the place, how it shows and whether it is private.
    pub fields: EventFields,
    /// The server's mark of the version it had after the change ([`Origin::change_key`]), and
    /// when it says that was (Unix seconds; 0 when it didn't say).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    #[serde(skip_serializing_if = "is_zero_time")]
    pub modified: Timestamp,
    /// How many downloads have shown the event otherwise since.
    #[serde(skip_serializing_if = "is_zero")]
    pub tries: u32,
    /// When the server said yes (Unix seconds, by the clock here).
    #[serde(skip_serializing_if = "is_zero_time")]
    pub made: Timestamp,
    /// For a deletion: the event as it was, which is what comes back should the server still
    /// have it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Box<Event>>,
    /// For the deletion of one occurrence of a series: the series, and when the series had it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub series: String,
    #[serde(skip_serializing_if = "is_zero_time")]
    pub occurrence: Timestamp,
    /// For a deletion: the server didn't say that it deleted the event, but that it has no such
    /// event. That is what it says of one that is gone already, and of one that was asked for
    /// at the wrong address: only a download tells which.
    #[serde(skip_serializing_if = "is_false")]
    pub not_found: bool,
    /// For the deletion that finishes a move to another calendar ([`EventChange::moved`]): what
    /// the server calls the copy, which goes again should the original turn out to be there
    /// still.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub moved: String,
    /// The account that has that copy, when it is not this check's ([`EventChange::copy_account`]).
    #[serde(skip_serializing_if = "is_no_id")]
    pub copy_account: Id,
}

impl EventCheck {
    /// It is about one occurrence of a series that was deleted, known by its series and its time.
    pub fn of_occurrence(&self) -> bool {
        self.remote.is_empty() && !self.series.is_empty()
    }
}

/// One calendar of an account that a host is asked to bring a stretch of.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CalendarAsk {
    /// What the server calls the calendar; empty for the account's main one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub calendar: String,
    /// The series the mailbox has in that calendar that should happen in the stretch, by what
    /// the server calls them: one the stretch brings nothing of is asked about by name.
    pub series: Vec<String>,
}

/// What stands before the address in the id of a calendar that was opened by address (Open
/// Shared Calendar, #70): its owner's address, and what it is read and written through
/// (`/users/{address}/calendar`). No server lists a calendar under such an id; one that does is
/// not believed ([`opened_address`]).
pub const OPENED_PREFIX: &str = "user:";

/// The id the calendar opened at `address` has here.
pub fn opened_id(address: &str) -> String {
    format!("{OPENED_PREFIX}{address}")
}

/// The address a calendar's id names, when it is one that was opened by address and that address
/// could name a mailbox to a server ([`crate::mailbox_address`]). `None` for every other id.
pub fn opened_address(id: &str) -> Option<String> {
    crate::mailbox_address(id.strip_prefix(OPENED_PREFIX)?)
}

/// Why a calendar that was asked for by address couldn't be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpenedWhy {
    /// The server says the account has no right to it (nothing is shared, or not with this
    /// account): a grant that was just made can take up to an hour to be there.
    #[default]
    NoRights,
    /// The server has nobody, or no mailbox, at that address.
    NoSuchPerson,
    /// The account's sign-in doesn't carry the permission to read other people's calendars.
    NoPermission,
    /// It couldn't be asked just now (no connection, busy): nothing is known.
    Unreachable,
}

/// A calendar opened by address that the server wouldn't give, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OpenedRefusal {
    pub address: String,
    pub why: OpenedWhy,
}

/// Most calendars one account is taken to have, and most categories a mailbox.
pub const MAX_REMOTE_CALENDARS: usize = 100;
pub const MAX_REMOTE_CATEGORIES: usize = 200;

/// A calendar as a server has it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteCalendar {
    /// What the server calls it; a calendar without that is not kept.
    pub id: String,
    pub name: String,
    /// Its colour there, when it has one of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// The name of the group it is listed under; empty for none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// What the account may do with it.
    pub rights: Rights,
    /// Whose it is, as the server says; `None` when it didn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<CalendarOwner>,
    /// It is the account's main calendar.
    pub main: bool,
    /// The services a meeting in it may be held online with, when the server said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online_providers: Option<Vec<OnlineProvider>>,
}

/// An account's calendars, as its server lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CalendarsDownload {
    pub account: Id,
    /// That account's address, as in [`EventsDownload::email`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    /// Every calendar the server has is here. When not, none is removed for not being here.
    pub complete: bool,
    pub calendars: Vec<RemoteCalendar>,
    /// Calendars the owner opened by address that the server wouldn't give (#70): one that was
    /// never read is taken off the list of those opened, with the reason said; one that was
    /// read before stays as it is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<OpenedRefusal>,
}

impl CalendarsDownload {
    /// The download as one for `account`.
    pub fn for_account(mut self, account: &Account) -> CalendarsDownload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }

    /// Cut to size: calendars without an id or said twice go, names are one line, the list is
    /// capped, and one calendar at most is the main one.
    pub fn tidy(&mut self) {
        let mut seen: Vec<String> = Vec::new();
        let mut main = false;
        self.refused.retain(|refusal| crate::mailbox_address(&refusal.address).is_some());
        self.refused.truncate(crate::MAX_OPENED_CALENDARS);
        let listed = std::mem::take(&mut self.calendars);
        for mut calendar in listed {
            calendar.id = key(&calendar.id);
            if calendar.id.is_empty() || seen.contains(&calendar.id) || seen.len() >= MAX_REMOTE_CALENDARS {
                continue;
            }
            seen.push(calendar.id.clone());
            calendar.name = line(&calendar.name, MAX_NAME);
            calendar.group = line(&calendar.group, MAX_NAME);
            calendar.owner = calendar.owner.as_ref().and_then(|owner| owner.cleaned());
            if let Some(allowed) = &mut calendar.online_providers {
                allowed.dedup();
                allowed.truncate(8);
            }
            calendar.main = calendar.main && !main;
            main |= calendar.main;
            self.calendars.push(calendar);
        }
    }
}

/// A mailbox's own categories with their colours, as its server has them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CategoriesDownload {
    pub account: Id,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub categories: Vec<Category>,
}

impl CategoriesDownload {
    /// The download as one for `account`.
    pub fn for_account(mut self, account: &Account) -> CategoriesDownload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }

    /// Cut to size: names are one line, none is empty or there twice, the list is capped.
    pub fn tidy(&mut self) {
        let mut kept: Vec<Category> = Vec::new();
        for category in std::mem::take(&mut self.categories) {
            let name = line(&category.name, MAX_NAME);
            if !name.is_empty() && kept.len() < MAX_REMOTE_CATEGORIES && !kept.iter().any(|have| have.name.eq_ignore_ascii_case(&name)) {
                kept.push(Category { name, color: category.color });
            }
        }
        self.categories = kept;
    }
}

/// Most folders one account's Favorites are taken to hold, and the deepest a folder among them
/// may lie (the names from the top of the mailbox down to it).
pub const MAX_FAVORITES: usize = 200;
pub const MAX_FAVORITE_DEPTH: usize = 32;

/// How reading an account's Favorites from its server went ([`FavoritesDownload`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FavoritesOutcome {
    /// The list was read: [`FavoritesDownload::folders`] is all of it, and may be empty.
    Read,
    /// The mailbox has no such list on its server. The mailbox's own Favorites are what there is.
    Missing,
    /// The sign-in doesn't cover reading it, or the server won't allow it.
    NotPermitted,
    /// It couldn't be read this time: nothing is concluded from that.
    #[default]
    Failed,
}

/// A folder that an account's server lists among the Favorites: where it is, so that the
/// mailbox can find its own folder for it. Nothing of what the server calls the folder (its id)
/// is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteFavorite {
    /// The names of the folders from the top of the mailbox down to it, itself last.
    pub path: Vec<String>,
    /// Which of the special folders it is (the Inbox whatever it is called there);
    /// [`FolderKind::Custom`] for any other.
    pub kind: FolderKind,
    /// Which of the special folders the first name of `path` is, for a folder that lies under
    /// one (a folder in the Inbox, whatever the Inbox is called there); [`FolderKind::Custom`]
    /// when it is none of them.
    #[serde(skip_serializing_if = "is_custom")]
    pub root: FolderKind,
}

fn is_custom(kind: &FolderKind) -> bool {
    *kind == FolderKind::Custom
}

/// An account's Favorites as its server has them (the folders Outlook shows under Favorites), in
/// the server's order, or why there are none to say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FavoritesDownload {
    pub account: Id,
    /// That account's address, as in [`EventsDownload::email`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub outcome: FavoritesOutcome,
    /// The folders, in the order the server has them. Only with [`FavoritesOutcome::Read`].
    pub folders: Vec<RemoteFavorite>,
    /// Entries of the server's list that are not folders (people, categories, groups): left out.
    pub skipped: u32,
    /// Entries that name a folder the mailbox doesn't have today (it was deleted, or belongs to
    /// another mailbox): left out.
    pub unresolved: u32,
}

impl FavoritesDownload {
    /// The download as one for `account`.
    pub fn for_account(mut self, account: &Account) -> FavoritesDownload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }

    /// Cut to size: names are one line, a folder without a name anywhere on its way is no
    /// folder, none is there twice, the list is capped; and only a list that was read has folders.
    pub fn tidy(&mut self) {
        let listed = std::mem::take(&mut self.folders);
        if self.outcome != FavoritesOutcome::Read {
            return;
        }
        for mut folder in listed {
            let deep = folder.path.len() > MAX_FAVORITE_DEPTH;
            folder.path = folder.path.iter().map(|name| line(name, MAX_NAME)).collect();
            // The special folders are found by what they are; any other by where it is.
            let nowhere = folder.path.is_empty() || folder.path.iter().any(String::is_empty) || deep;
            if nowhere && folder.kind == FolderKind::Custom {
                continue;
            }
            if nowhere {
                folder.path.clear();
            }
            if folder.kind != FolderKind::Custom && folder.path.len() <= 1 {
                folder.root = folder.kind;
            }
            let same = |have: &RemoteFavorite| {
                (have.kind != FolderKind::Custom && have.kind == folder.kind)
                    || (have.kind == folder.kind
                        && have.root == folder.root
                        && have.path.len() == folder.path.len()
                        && have.path.iter().zip(&folder.path).all(|(a, b)| a.to_lowercase() == b.to_lowercase()))
            };
            if self.folders.len() < MAX_FAVORITES && !self.folders.iter().any(same) {
                self.folders.push(folder);
            }
        }
    }
}

/// Most people whose time is asked about at once, and the longest stretch (seconds): what a
/// server answers in one go.
pub const MAX_SCHEDULES: usize = 20;
pub const MAX_SCHEDULE_STRETCH: i64 = 62 * 86_400;
/// Most stretches of time taken of one person.
pub const MAX_BUSY: usize = 2_000;

/// Whose time is asked about, and when ([`FreeBusy`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FreeBusyAsk {
    /// Their addresses.
    pub people: Vec<String>,
    /// The stretch (Unix seconds): from, and up to.
    pub from: Timestamp,
    pub to: Timestamp,
}

impl FreeBusyAsk {
    /// Cut to what a server is asked: addresses that are addresses, each once, a few at most.
    pub fn tidy(&mut self) {
        let mut people: Vec<String> = Vec::new();
        for email in self.people.iter().map(|email| mailbox(email)).filter(|email| !email.is_empty()) {
            if people.len() < MAX_SCHEDULES && !people.iter().any(|have| have.eq_ignore_ascii_case(&email)) {
                people.push(email);
            }
        }
        self.people = people;
    }
}

/// A stretch of somebody's time that is taken, and how.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BusyTime {
    pub start: Timestamp,
    pub end: Timestamp,
    pub show_as: ShowAs,
}

/// What a server says of one person's time. Nothing of what the time is taken by: only when.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PersonSchedule {
    pub email: String,
    pub busy: Vec<BusyTime>,
    /// Why nothing is known of this person (no such mailbox, not allowed to see); empty otherwise.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// When the people who were asked about are busy, as a server says ([`FreeBusyAsk`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FreeBusy {
    pub from: Timestamp,
    pub to: Timestamp,
    pub people: Vec<PersonSchedule>,
}

impl FreeBusy {
    /// Cut to size: people with an address, each once; stretches with a length, a number of them.
    pub fn tidy(&mut self) {
        let mut people: Vec<PersonSchedule> = Vec::new();
        for mut person in std::mem::take(&mut self.people) {
            person.email = mailbox(&person.email);
            if person.email.is_empty() || people.len() >= MAX_SCHEDULES || people.iter().any(|have| have.email.eq_ignore_ascii_case(&person.email)) {
                continue;
            }
            person.error = line(&person.error, MAX_WHY);
            person.busy.retain(|time| time.end > time.start);
            person.busy.truncate(MAX_BUSY);
            people.push(person);
        }
        self.people = people;
    }
}

/// What a server calls something, when `text` could be that: an id, a change key
/// ([`MAX_KEY`] bytes at most, nothing a line or a word ends at). Nothing otherwise.
pub fn clean_key(text: &str) -> String {
    key(text)
}

/// The mark of a new event, when `text` can be one: letters, digits and hyphens.
fn mark(text: &str) -> String {
    let text = text.trim();
    let fits = !text.is_empty() && text.len() <= MAX_TOKEN && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if fits { text.to_string() } else { String::new() }
}

/// An id or a change key from outside: as it came, or nothing when it couldn't be one.
pub(crate) fn key(text: &str) -> String {
    let text = text.trim();
    if text.len() > MAX_KEY || text.chars().any(|c| c.is_control() || c.is_whitespace()) { String::new() } else { text.to_string() }
}

/// One line of text from outside: line breaks and control characters read as spaces, and it is
/// at most `max` characters long.
pub(crate) fn line(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let short: String = flat.trim().chars().take(max).collect();
    short.trim_end().to_string()
}

/// Text of several lines from outside: its line ends are `\n`, it has no other control
/// characters, and it is at most `max` characters long.
pub(crate) fn text(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).take(max).collect();
    clean.trim().to_string()
}

/// An e-mail address from outside, or nothing when it isn't one (a server may name people of its
/// own directory in other ways).
fn mailbox(email: &str) -> String {
    let email = email.trim();
    let shaped = email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
    let plain = email.len() <= 254 && !email.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>' | ',' | ';' | '"'));
    if shaped && plain { email.to_string() } else { String::new() }
}

/// The people a meeting is forwarded to: each with an address that is one, nobody twice,
/// [`MAX_FORWARD`] at most.
fn recipients(list: Vec<Address>) -> Vec<Address> {
    let mut out: Vec<Address> = Vec::new();
    for who in list.into_iter().filter_map(person).filter(|who| !who.email.is_empty()) {
        if out.len() < MAX_FORWARD && !out.iter().any(|have| have.email.eq_ignore_ascii_case(&who.email)) {
            out.push(who);
        }
    }
    out
}

/// [`recipients`], for whoever notes a forward: what a server would be sent of `list`.
pub fn forward_recipients(list: Vec<Address>) -> Vec<Address> {
    recipients(list)
}

/// Somebody an event names: with a name of one line and an address that is one. `None` when
/// neither is left.
fn person(address: Address) -> Option<Address> {
    let person = Address { name: line(&address.name, MAX_LINE), email: mailbox(&address.email) };
    (!person.name.is_empty() || !person.email.is_empty()).then_some(person)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::{Calendar, Contact, Event};

    #[test]
    fn an_item_of_this_mailbox_reads_and_writes_as_before() {
        // Nothing about a server is written for what lives here only.
        for json in [
            serde_json::to_string(&Event::default()).unwrap(),
            serde_json::to_string(&Contact::default()).unwrap(),
            serde_json::to_string(&Calendar::default()).unwrap(),
        ] {
            assert!(!json.contains("origin"), "{json}");
        }
        // And what was written before there was any of this still reads.
        let event: Event = serde_json::from_str(r#"{"id":7,"calendar":2,"title":"Lunch"}"#).unwrap();
        assert!(event.origin.is_local() && !event.origin.is_remote());
        let contact: Contact = serde_json::from_str(r#"{"id":8,"first":"Ada"}"#).unwrap();
        assert_eq!(contact.origin, Origin::default());
        let calendar: Calendar = serde_json::from_str(r#"{"id":2,"name":"Calendar","visible":true}"#).unwrap();
        assert!(calendar.origin.is_local());
    }

    #[test]
    fn where_an_item_came_from_is_kept_with_it() {
        let origin =
            Origin { account: 3, id: "AAMk-1=".into(), change_key: "ck/1==".into(), series: "AAMk-0=".into(), guest: false, occurrence: None };
        let event = Event { id: 7, title: "Review".into(), origin: origin.clone(), ..Default::default() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""origin":{"account":3,"id":"AAMk-1=","changeKey":"ck/1==","series":"AAMk-0="}"#), "{json}");
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
        // Only what there is of it is written.
        let calendar = Calendar { id: 9, origin: Origin { account: 3, ..Default::default() }, ..Default::default() };
        let json = serde_json::to_string(&calendar).unwrap();
        assert!(json.contains(r#""origin":{"account":3}"#), "{json}");
        assert_eq!(serde_json::from_str::<Calendar>(&json).unwrap(), calendar);
        let contact = Contact { id: 4, origin: Origin { account: 3, id: "c-1".into(), ..Default::default() }, ..Default::default() };
        assert_eq!(serde_json::from_str::<Contact>(&serde_json::to_string(&contact).unwrap()).unwrap(), contact);
        // An id without an account is nobody's.
        assert!(Origin { id: "x".into(), ..Default::default() }.is_local());
    }

    #[test]
    fn downloads_read_and_write_as_json() {
        let account = Account { id: 3, email: " ada@example.org ".into(), ..Default::default() };
        let events = EventsDownload {
            from: 100,
            to: 200,
            complete: true,
            events: vec![RemoteEvent {
                id: "e-1".into(),
                title: "Review".into(),
                start: 120,
                end: 150,
                attendees: vec![Attendee { address: Address::new("Bo", "bo@example.org"), optional: true, response: Response::Accepted }],
                kind: RemoteEventKind::Exception,
                ..Default::default()
            }],
            ..Default::default()
        }
        .for_account(&account);
        assert_eq!((events.account, events.email.as_str()), (3, "ada@example.org"));
        let json = serde_json::to_string(&events).unwrap();
        assert!(json.contains(r#""kind":"exception""#) && json.contains(r#""complete":true"#) && json.contains(r#""allDay":false"#), "{json}");
        assert_eq!(serde_json::from_str::<EventsDownload>(&json).unwrap(), events);
        let contacts = ContactsDownload {
            complete: true,
            contacts: vec![RemoteContact { id: "c-1".into(), first: "Bo".into(), ..Default::default() }],
            ..Default::default()
        }
        .for_account(&account);
        assert_eq!(serde_json::from_str::<ContactsDownload>(&serde_json::to_string(&contacts).unwrap()).unwrap(), contacts);
        // Whatever is left out reads as nothing, and nothing is then complete.
        let bare: EventsDownload = serde_json::from_str(r#"{"events":[{}]}"#).unwrap();
        assert_eq!((bare.complete, bare.account, bare.events.len()), (false, 0, 1));
    }

    #[test]
    fn events_from_outside_are_cut_to_size() {
        let mut event = RemoteEvent {
            id: "  AAMk-1=  ".into(),
            change_key: "two words".into(),
            series: "x".repeat(MAX_KEY + 1),
            uid: "uid\r\n1".into(),
            title: " Budget\r\nreview\u{0} ".into(),
            location: "x".repeat(2 * MAX_LINE),
            description: "Line one\r\nLine two\u{7}\ttabbed\n".into(),
            start: 500,
            end: 100,
            organizer: Some(Address { name: "\u{0}".into(), email: "not an address".into() }),
            attendees: vec![
                Attendee { address: Address { name: "Bo\nBcc: eve@example.org".into(), email: " bo@example.org ".into() }, ..Default::default() },
                Attendee { address: Address { name: "Room 4".into(), email: "/o=Example/cn=Room 4".into() }, optional: true, ..Default::default() },
                Attendee { address: Address { name: String::new(), email: "a@b@c".into() }, ..Default::default() },
            ],
            reminder_minutes: Some(u32::MAX),
            ..Default::default()
        };
        event.tidy();
        assert_eq!((event.id.as_str(), event.change_key.as_str(), event.series.as_str(), event.uid.as_str()), ("AAMk-1=", "", "", "uid  1"));
        assert_eq!(event.title, "Budget  review");
        assert_eq!(event.location.chars().count(), MAX_LINE);
        assert_eq!(event.description, "Line one\nLine two\ttabbed");
        assert_eq!((event.start, event.end), (500, 500));
        assert_eq!(event.organizer, None);
        // An address is one or it is left out; somebody with a name stays, nobody goes.
        assert_eq!(
            event.attendees,
            [
                Attendee { address: Address { name: "Bo Bcc: eve@example.org".into(), email: "bo@example.org".into() }, ..Default::default() },
                Attendee { address: Address { name: "Room 4".into(), email: String::new() }, optional: true, ..Default::default() },
            ]
        );
        assert_eq!(event.reminder_minutes, Some(MAX_REMINDER));
        let once = event.clone();
        event.tidy();
        assert_eq!(event, once);

        let mut crowded = RemoteEvent {
            attendees: (0..2 * MAX_ATTENDEES)
                .map(|n| Attendee { address: Address::new("", format!("p{n}@example.org")), ..Default::default() })
                .collect(),
            description: "é".repeat(2 * MAX_TEXT),
            ..Default::default()
        };
        crowded.tidy();
        assert_eq!((crowded.attendees.len(), crowded.description.chars().count()), (MAX_ATTENDEES, MAX_TEXT));
    }

    #[test]
    fn contacts_from_outside_are_cut_to_size() {
        let mut contact = RemoteContact {
            id: "c\u{0}1".into(),
            display_name: " Ada\tLovelace \r\n".into(),
            emails: vec![Labeled::new("work\n", " ada@example.org "), Labeled::new("", "no address"), Labeled::new("", "")],
            phones: vec![Labeled::new("mobile", " +1 555\u{0}0100 "), Labeled::new("home", "  ")],
            addresses: vec![
                PostalAddress { label: "work".into(), street: "1 Engine Way\r\nSuite 2".into(), city: "London\n".into(), ..Default::default() },
                PostalAddress { label: "home".into(), ..Default::default() },
            ],
            websites: vec!["https://example.org/ada\r\n".into(), " ".into()],
            notes: "First\r\nSecond\u{1b}[31m".into(),
            ..Default::default()
        };
        contact.tidy();
        assert_eq!((contact.id.as_str(), contact.display_name.as_str()), ("", "Ada Lovelace"));
        assert_eq!(contact.emails, [Labeled::new("work", "ada@example.org")]);
        assert_eq!(contact.phones, [Labeled::new("mobile", "+1 555 0100")]);
        assert_eq!(contact.addresses.len(), 1);
        assert_eq!((contact.addresses[0].street.as_str(), contact.addresses[0].city.as_str()), ("1 Engine Way\nSuite 2", "London"));
        assert_eq!(contact.websites, ["https://example.org/ada"]);
        assert_eq!(contact.notes, "First\nSecond[31m");
        let once = contact.clone();
        contact.tidy();
        assert_eq!(contact, once);

        // Entries with nothing in them don't count towards how many are kept.
        let mut crowded = RemoteContact {
            emails: std::iter::once(Labeled::new("", "")).chain((0..200).map(|n| Labeled::new("", format!("a{n}@example.org")))).collect(),
            websites: vec!["x".repeat(10_000); 200],
            birthday: Some(Date { year: 2026, month: 2, day: 30 }),
            ..Default::default()
        };
        crowded.tidy();
        assert_eq!((crowded.emails.len(), crowded.websites.len(), crowded.websites[0].len()), (MAX_ENTRIES, MAX_ENTRIES, MAX_LINE));
        assert_eq!((crowded.emails[0].value.as_str(), crowded.birthday), ("a0@example.org", None));
        // A day that is one stays.
        let mut born = RemoteContact { birthday: Date::new(1815, 12, 10), ..Default::default() };
        born.tidy();
        assert_eq!(born.birthday, Date::new(1815, 12, 10));
    }

    proptest! {
        #[test]
        fn tidying_any_text_never_panics_and_settles(a in any::<String>(), b in "\\PC{0,40}", n in any::<i64>(), m in any::<i64>()) {
            let mut event = RemoteEvent {
                id: a.clone(),
                change_key: b.clone(),
                uid: a.clone(),
                title: a.clone(),
                location: b.clone(),
                description: a.clone(),
                start: n,
                end: m,
                organizer: Some(Address { name: a.clone(), email: b.clone() }),
                attendees: vec![Attendee { address: Address { name: b.clone(), email: a.clone() }, ..Default::default() }],
                ..Default::default()
            };
            event.tidy();
            prop_assert!(event.end >= event.start);
            prop_assert!(!event.title.chars().any(char::is_control) && !event.id.chars().any(char::is_whitespace));
            prop_assert!(event.attendees.iter().all(|x| x.address.email.is_empty() || x.address.email.contains('@')));
            let once = event.clone();
            event.tidy();
            prop_assert_eq!(event, once);

            let mut contact = RemoteContact {
                id: b.clone(),
                display_name: a.clone(),
                notes: a.clone(),
                emails: vec![Labeled::new(a.clone(), b.clone()), Labeled::new(b.clone(), a.clone())],
                phones: vec![Labeled::new(a.clone(), a.clone())],
                addresses: vec![PostalAddress { street: a.clone(), city: b.clone(), ..Default::default() }],
                websites: vec![a, b],
                ..Default::default()
            };
            contact.tidy();
            prop_assert!(contact.emails.iter().all(|e| e.value.contains('@')));
            let once = contact.clone();
            contact.tidy();
            prop_assert_eq!(contact, once);
        }
    }
}
