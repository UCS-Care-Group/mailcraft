// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in for Microsoft Graph's calendar that behaves like the documented service, for the
//! tests. Nothing here leaves this machine, and nothing here is in a build that isn't a test's.
//!
//! The other stand-ins of the tests answer whatever they are told to and take whatever they are
//! sent. This one keeps a calendar and holds a client to the contract of Graph v1.0 as its
//! documentation has it:
//!
//! - **Addresses.** `/v1.0/me/events`, `/me/events/{id}`, `/me/calendars/{id}/events[/{id}]`,
//!   `/me/calendarView`, `/me/calendars/{id}/calendarView`, `/me/events/{id}/instances`, the
//!   actions `cancel`, `accept`, `tentativelyAccept`, `decline` and `forward`, `/me/calendars`,
//!   `/me/calendarGroups`, and `/$batch`. An id is one piece of an address: ids here are long
//!   and hold `=`, `-`, `_`, `/` and `+`, so one that isn't written as `%XX` where it must be
//!   names nothing (404, or 400). An event of a calendar that somebody else shares is found
//!   through that calendar only.
//! - **Headers.** `Authorization: Bearer` with the token it was set up with (401 otherwise), a
//!   JSON `Content-Type` with every body (415 otherwise). `Prefer: outlook.timezone` and
//!   `outlook.body-content-type` are honoured: times come back in UTC, and a body as HTML unless
//!   text was asked for.
//! - **Bodies.** Only the properties an event has, each of its type; `dateTime` as
//!   `2026-10-15T16:00:00` with a `timeZone` it knows; a whole-day event from midnight to
//!   midnight in one zone, a day at least; an end not before the start; a recurrence with the
//!   properties its pattern needs and a range that starts on the day the event starts;
//!   `transactionId` only with a new event. What isn't so is answered 400 with an error as Graph
//!   words them (`{"error": {"code", "message", "innerError"}}`).
//! - **Meetings.** `importance`, `responseRequested`, `isOnlineMeeting` and
//!   `onlineMeetingProvider` are taken with a new event and with a change;
//!   `allowNewTimeProposals` with a new event only (the reference doesn't list it among what an
//!   update may say). An event that is made an online meeting gets its `onlineMeeting.joinUrl`
//!   and stays one: later words about it are ignored. `tentativelyAccept` and `decline` take a
//!   `proposedNewTime` (a start and an end, each a `dateTimeTimeZone`) and answer 400 when the
//!   event doesn't allow proposals or `sendResponse` is false; `accept` has no such parameter.
//!   `forward` takes `ToRecipients` and `Comment` and answers 202.
//! - **Answers.** 201 with the event for one that was made, 200 with the event for a change, 204
//!   for a deletion, 202 for an action, lists in pages with `@odata.nextLink`. A calendar view
//!   holds single events, the occurrences of series and their exceptions, never a series itself.
//!   A changed occurrence becomes an exception under the id it had.
//! - **Folders of mail and the Favorites.** `GET /v1.0/me/mailFolders/{id}` answers a folder by
//!   its id or by a well-known name (`inbox`, `sentitems`, `msgfolderroot`…), with `$select` of a
//!   mail folder's own properties and no other parameter; an id is written in Graph's alphabet
//!   (`-` and `_`, never `/` or `+`: 400), and one that names nothing is 404. In a `$batch` a
//!   request that is told to come back later (429) says when among its own headers
//!   (`Retry-After`), as the reference has it ([`State::throttle_folders`]).
//!   `GET /beta/me/mailFolders/root/userConfigurations/Aggregated.OwaUserConfiguration` answers
//!   the cached configuration of Outlook on the web as `{"id", "xmlData"}` (base64): there is no
//!   list of such items (400), no query parameters (400), nothing of it in v1.0, and it is only
//!   read (403 to anything else). What the document holds is nobody's documentation: it is made
//!   here as it was once seen ([`State::configuration_xml`]), with knobs for a mailbox that has
//!   none ([`Configuration::Missing`]), a sign-in that doesn't cover it
//!   ([`Configuration::Denied`], [`State::folders_denied`]), entries for folders that are gone
//!   or belong to another mailbox, entries that are no folders, and a document that is anything
//!   at all ([`State::configuration_document`]).
//! - **Other mailboxes** (#70). `/v1.0/users/{address}/mailFolders…`,
//!   `/users/{address}/messages[/{id}[/$value|/move|/copy|/send|/createReply…|/attachments]]` and
//!   `/users/{address}/sendMail` answer a shared mailbox's folders, drafts, sending and
//!   mail as `/me` answers the owner's, its changes made in it alone ([`State::add_other`]); 403 when the owner wasn't given Full Access to
//!   it, 404 for an address it doesn't know.
//! - **Search** (R6.2). `$search` on `/me/messages` and `/me/mailFolders/{id}/messages` (and a
//!   shared mailbox's), as the reference has it for messages: [`search`].
//! - **Messages.** A folder's message delta in pages, with next and delta links, `@removed`,
//!   replays, forgotten and reset links, and a message's MIME (`$value`): [`messages`]. Changes
//!   of a message (read, flag, categories, importance, extended properties), move, copy and
//!   delete, a folder's messages found by an extended property, and faults on demand per
//!   message (404, 403, 400, 429, an answer lost): [`changes`].
//! - **Messages made in a folder** (#70): `POST /me/mailFolders/{id}/messages` (and a shared
//!   mailbox's) makes a message there from JSON, as a draft unless `PidTagMessageFlags` written
//!   with it say it isn't unsent ([`State::drafts_only`] makes it a draft whatever they say);
//!   its sender, read state, flag, and the time it was received (`PidTagMessageDeliveryTime`)
//!   and its `Message-ID` (`PidTagInternetMessageId`) as written.
//! - **Drafts and attachments.** Drafts made from JSON or MIME, answers and forwards made with
//!   create-reply, create-reply-all and create-forward, a draft's subject, body and recipients
//!   changed in place, attachments added, listed and deleted, large ones through upload
//!   sessions, and answers lost on demand ([`State::lose_next`]): [`drafts`].
//! - **Time.** [`State::stale_views`] calendar views after a change are answered as the calendar
//!   was before it: an answer may be older than the last change. [`State::keeps_times`] makes it
//!   say yes to a change of time and not make it, which no client should take for done.
//!
//! [`route`] has the calls of this crate that go to Graph go to a stand-in instead, on the
//! thread that asks for it and until the guard is dropped.

mod changes;
mod drafts;
mod messages;
mod search;
mod send;

pub use changes::Fault;
pub use drafts::Attached;
pub use messages::{Mail, Message};
pub use send::Sent;

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};

use mailcraft_time::{DAY, Date, DateTime, Timestamp, Weekday};
use serde_json::{Map, Value, json};

/// The token a stand-in takes unless it is told another.
pub const TOKEN: &str = "standin.acc-ess_tok.en-0123456789";
/// What stands, in the cached configuration of Outlook on the web, for everything in it that
/// is not the list of Favorites: nothing a client reads, keeps, shows or logs may hold it.
pub const CONFIGURATION_CANARY: &str = "not-for-anybody-7c1e";
/// What a folder of mail may be asked for by name (`$select`).
const MAIL_FOLDER_PROPERTIES: &[&str] =
    &["id", "displayName", "parentFolderId", "childFolderCount", "unreadItemCount", "totalItemCount", "sizeInBytes", "isHidden"];
/// Largest request it reads, and how many entries a page of a list has when none is asked for.
const MAX_REQUEST: usize = 5 * 1024 * 1024;
const PAGE: usize = 10;
/// Most requests kept for a test to look at.
const MAX_KEPT: usize = 20_000;
/// Most occurrences of one series in one answer.
const MAX_OCCURRENCES: usize = 2_000;

thread_local! {
    static ROUTE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Where this thread's calls to Graph go instead, when a test has said so ([`route`]).
pub(crate) fn routed() -> Option<String> {
    ROUTE.with(|route| route.borrow().clone())
}

/// A stand-in is being spoken to in Graph's place on this thread, for as long as this lives.
pub struct Routed(Option<String>);

/// Have this thread's calls to Graph go to `base` (`http://127.0.0.1:{port}`, a stand-in on
/// this machine: nothing else is spoken to unencrypted) until the guard is dropped.
pub fn route(base: &str) -> Routed {
    Routed(ROUTE.with(|route| route.borrow_mut().replace(base.to_string())))
}

impl Drop for Routed {
    fn drop(&mut self) {
        let before = self.0.take();
        ROUTE.with(|route| *route.borrow_mut() = before);
    }
}

/// A calendar the stand-in has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    /// The account's main one.
    pub main: bool,
    /// The account may write to it.
    pub can_edit: bool,
    /// Somebody else shares it: its events are found through it only.
    pub shared: bool,
    /// The services a meeting in it may be held online with (`allowedOnlineMeetingProviders`),
    /// the first of them by default.
    pub online: Vec<String>,
}

/// An event as the stand-in keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stored {
    pub id: String,
    pub calendar: String,
    /// Counts its versions: the change key is made of it.
    pub version: u32,
    pub uid: String,
    pub subject: String,
    pub location: String,
    pub body: String,
    /// Instants (Unix seconds); for a whole-day event the midnights in `zone`.
    pub start: Timestamp,
    pub end: Timestamp,
    pub all_day: bool,
    /// The zone its start was written in.
    pub zone: String,
    /// The zone its end was written in, when that is another ([`Stored::end_zone`]).
    pub ends_in: String,
    pub show_as: String,
    pub sensitivity: String,
    pub reminder_on: bool,
    pub reminder_minutes: u64,
    pub attendees: Vec<Value>,
    pub categories: Vec<String>,
    pub recurrence: Option<Value>,
    pub transaction_id: String,
    pub is_organizer: bool,
    /// What the owner answered, for somebody else's meeting.
    pub response: String,
    /// For an exception: its series, and when the series had it.
    pub series: String,
    pub original: Timestamp,
    /// For a series: the occurrences that were deleted or became exceptions, by when the series
    /// had them.
    pub gone: Vec<Timestamp>,
    pub modified: Timestamp,
    /// `low` or `high`; empty for `normal`.
    pub importance: String,
    /// The people invited are not asked to answer (`responseRequested` is false), and may not
    /// propose another time (`allowNewTimeProposals` is false). Both are true unless said.
    pub no_response_requested: bool,
    pub no_new_times: bool,
    /// It is an online meeting, held with this, and this is the link to join it by.
    pub online: bool,
    pub online_provider: String,
    pub join_url: String,
}

/// A folder of mail the stand-in has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailFolder {
    pub id: String,
    pub name: String,
    /// The folder it lies in, by its id.
    pub parent: String,
    /// The well-known name it also answers to (`inbox`, `sentitems`, `msgfolderroot`…); empty
    /// for a folder that has none.
    pub well_known: String,
    /// It is hidden (`isHidden`): a list leaves it out unless hidden folders are asked for.
    pub hidden: bool,
    /// It is a search folder (`#microsoft.graph.mailSearchFolder`).
    pub search: bool,
}

/// One entry of the Favorites, as the cached configuration of Outlook on the web lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Favorite {
    /// The name it is shown by there (which may be long out of date).
    pub name: String,
    /// Its place in the list; `None` writes none.
    pub index: Option<i64>,
    /// `Folder` for a folder; anything else for what is none.
    pub kind: String,
    /// The folder's id as that document writes it (with `/` and `+`); empty writes no such setting.
    pub folder_id: String,
}

/// A mailbox other than the owner's (#70: a shared mailbox, `support@`), answered at
/// `/v1.0/users/{address}/mailFolders…` and `/v1.0/users/{address}/messages/{id}/$value` as the
/// owner's own is at `/me/…`: its own folders, messages and rounds of delta. Its ids are made as
/// the owner's are, so the same id may name a message in each.
#[derive(Clone, Debug, Default)]
pub struct OtherMailbox {
    pub address: String,
    /// The owner of the token was given Full Access to it; otherwise every question about it is
    /// answered 403 (`ErrorAccessDenied`), as Exchange does.
    pub full_access: bool,
    /// What every question about it is answered instead, when set: a status and Graph's code
    /// (a 403 `ErrorQuotaExceeded`, a 404 `MailboxNotEnabledForRESTAPI`…), to tell 403s apart.
    pub refusing: Option<(u16, &'static str)>,
    /// Exchange doesn't let the owner of the token send as it, nor on its behalf: a send from it
    /// is answered 403 `ErrorSendAsDenied`, and nothing goes (Full Access alone can't send).
    pub send_as_denied: bool,
    pub folders: Vec<MailFolder>,
    pub mailbox: Mail,
}

/// The special folders every mailbox has under its top, by their names and well-known names.
const SPECIAL_FOLDERS: [(&str, &str); 7] = [
    ("Inbox", "inbox"),
    ("Drafts", "drafts"),
    ("Sent Items", "sentitems"),
    ("Deleted Items", "deleteditems"),
    ("Junk Email", "junkemail"),
    ("Archive", "archive"),
    ("Outbox", "outbox"),
];

/// Whether the mailbox has the cached configuration of Outlook on the web, and may be asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Configuration {
    /// It is there.
    #[default]
    There,
    /// The mailbox has no such item: 404.
    Missing,
    /// The sign-in doesn't cover reading it: 403.
    Denied,
}

/// One request the stand-in was sent, with the status it answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub method: String,
    /// The address as it came, with its query.
    pub target: String,
    /// The pieces of the path, each as it reads once its `%XX` are undone.
    pub path: Vec<String>,
    pub query: Vec<(String, String)>,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// The body as it came, byte for byte (the piece of an upload). Not kept with the requests.
    pub bytes: Vec<u8>,
    pub status: u16,
}

impl Asked {
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(have, _)| have == name).map_or("", |(_, value)| value.as_str())
    }

    fn param(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(have, _)| have == name).map(|(_, value)| value.as_str())
    }

    /// The body as JSON; `Null` when it isn't.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// What the stand-in keeps, for a test to set up and to look at.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The token it takes.
    pub token: String,
    pub calendars: Vec<Calendar>,
    pub events: Vec<Stored>,
    /// Every request that came, in order.
    pub requests: Vec<Asked>,
    /// How many calendar views to come are answered as the calendar was before its last change.
    pub stale_views: usize,
    /// It says yes to a change of an event's time and leaves the time as it was.
    pub keeps_times: bool,
    /// What it answers to the next requests of a method instead of doing them: the method, the
    /// status and the name of the error. (A method may be followed by a path, `POST /v1.0/me/mailFolders`,
    /// for the requests of that method to that address only.)
    pub fail_next: Vec<(String, u16, String)>,
    /// It lets a change say whether another time may be proposed (`allowNewTimeProposals`),
    /// which the reference lists for a new event and not for an update. Otherwise a change that
    /// says it is answered 400. (The service may do either: a client has to live with both.)
    pub updates_new_times: bool,
    /// An event that is to be held online with a service its calendar doesn't allow is made
    /// without it, and nothing is said. Otherwise it is answered 400. (The reference doesn't
    /// say which of the two the service does: a client has to live with both.)
    pub drops_online: bool,
    /// A folder is listed without its `totalItemCount` and `unreadItemCount` (#276).
    pub uncounted: bool,
    /// The mail a real server would have sent to other people: `invitation:Title:people`,
    /// `update:Title:people`, `cancellation:Title:people:note`, `response:Title:answer:note`,
    /// `proposal:Title:start/end` (with a response that proposes another time) and
    /// `forward:Title:people:note`.
    pub mail: Vec<String>,
    /// The owner's address, which is who organizes what is made here.
    pub owner: String,
    /// The folders of mail: the top of the mailbox, the special folders under it, and whatever
    /// a test adds ([`State::add_folder`]).
    pub folders: Vec<MailFolder>,
    /// The Favorites the cached configuration of Outlook on the web lists; `None` for a
    /// configuration that has no list of them at all.
    pub favorites: Option<Vec<Favorite>>,
    /// Whether the mailbox has that configuration, and whether it may be read.
    pub configuration: Configuration,
    /// The document that configuration holds, when it is to be something else than the one made
    /// of `favorites` (too large, cut short, not XML at all): served as it is.
    pub configuration_document: Option<String>,
    /// The sign-in doesn't cover reading folders: 403 to every question about one.
    pub folders_denied: bool,
    /// How many questions about a folder to come are told to come back later (429, with a
    /// `Retry-After`) instead of being answered.
    pub throttle_folders: usize,
    /// How many seconds a request that is told to come back later is told to wait (`Retry-After`);
    /// `None` for one.
    pub retry_after: Option<u32>,
    /// Lists of folders include hidden ones even when they aren't asked for (`includeHiddenFolders`).
    /// (A client has to leave them out either way.)
    pub lists_hidden: bool,
    /// Most folders a page of a list of folders holds, whatever `$top` asked for; 0 for as asked.
    pub folder_page: usize,
    /// The folders whose list of subfolders can't be had just now (500): a walk of the folders is
    /// cut short there.
    pub broken_lists: Vec<String>,
    /// The prefixes the document writes its two namespaces with (`a` and `b` unless said).
    pub prefixes: (String, String),
    /// The messages of every folder, and the rounds of their delta ([`messages`]).
    pub mailbox: Mail,
    /// The mailboxes other than the owner's that it knows (#70); any other address is 404
    /// (`ErrorInvalidUser`).
    pub others: Vec<OtherMailbox>,
    /// Whose mailbox is being answered for: empty for the owner's (`me`), else
    /// `users/{address}` as the links it writes name it.
    at: String,
    /// Requests to carry out and then lose the answer to, one each: a method and an address,
    /// `*` for any one piece of it (`POST /v1.0/me/messages`, `PUT /upload/*`).
    pub lose_next: Vec<String>,
    /// A message made in a folder from JSON is a draft whatever its `PidTagMessageFlags` say, as
    /// the reference has every message made so (#70). Otherwise flags written with it that say
    /// it isn't unsent make it received mail, as Exchange does.
    pub drafts_only: bool,
    /// What it sent, in order ([`send`]).
    pub sent: Vec<Sent>,
    /// What every search of messages is answered with instead (a status and Graph's code), when
    /// set ([`search`]).
    pub search_refused: Option<(u16, &'static str)>,
    /// How many searches of messages it answered.
    pub searches: usize,
    /// The `Location` header of the answer being made, when it has one.
    location: Option<String>,
    /// The answer being made when it isn't JSON: its type and its body.
    raw: Option<(String, String)>,
    /// The request was carried out and its answer is to be lost: the connection is closed
    /// without a word ([`Fault::AnswerLost`]).
    lose_answer: bool,
    before: Option<Vec<Stored>>,
    made: u64,
    clock: Timestamp,
    base: String,
}

/// A refusal: the status, Graph's name for it, and its words.
type Refusal = (u16, &'static str, String);
/// An answer: the status and, when there is one, the body.
type Answer = (u16, Option<Value>);

fn refuse(status: u16, code: &'static str, message: impl Into<String>) -> Refusal {
    (status, code, message.into())
}

fn bad(message: impl Into<String>) -> Refusal {
    refuse(400, "ErrorInvalidRequest", message)
}

fn not_found() -> Refusal {
    refuse(404, "ErrorItemNotFound", "The specified object was not found in the store.")
}

/// The days of the week as Graph names them, Monday first.
const DAY_NAMES: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
/// What an event may be asked for by name (`$select`), and what may be written of it.
const PROPERTIES: &[&str] = &[
    "id",
    "createdDateTime",
    "lastModifiedDateTime",
    "changeKey",
    "categories",
    "transactionId",
    "originalStartTimeZone",
    "originalEndTimeZone",
    "iCalUId",
    "uid",
    "reminderMinutesBeforeStart",
    "isReminderOn",
    "hasAttachments",
    "subject",
    "bodyPreview",
    "importance",
    "sensitivity",
    "isAllDay",
    "isCancelled",
    "isOrganizer",
    "responseRequested",
    "seriesMasterId",
    "showAs",
    "type",
    "webLink",
    "onlineMeetingUrl",
    "isOnlineMeeting",
    "onlineMeetingProvider",
    "allowNewTimeProposals",
    "occurrenceId",
    "isDraft",
    "hideAttendees",
    "responseStatus",
    "body",
    "start",
    "end",
    "location",
    "locations",
    "recurrence",
    "attendees",
    "organizer",
    "onlineMeeting",
    "originalStart",
    "cancelledOccurrences",
    "exceptionOccurrences",
    "calendar",
    "instances",
    "attachments",
];
const WRITABLE: &[&str] = &[
    "subject",
    "body",
    "start",
    "end",
    "isAllDay",
    "location",
    "locations",
    "attendees",
    "categories",
    "showAs",
    "sensitivity",
    "importance",
    "isReminderOn",
    "reminderMinutesBeforeStart",
    "recurrence",
    "responseRequested",
    "isOnlineMeeting",
    "onlineMeetingProvider",
    "hideAttendees",
];

/// The names a `dateTimeTimeZone` is documented to take beside the ones Windows has for its
/// zones: the "additional time zones" of the reference, as it lists them. (Where it gives two
/// spellings of a name, both are here.)
const ADDITIONAL_ZONES: &[&str] = &[
    "Etc/GMT+12",
    "Etc/GMT+11",
    "Pacific/Honolulu",
    "America/Anchorage",
    "America/Santa_Isabel",
    "America/Los_Angeles",
    "America/Phoenix",
    "America/Chihuahua",
    "America/Denver",
    "America/Guatemala",
    "America/Chicago",
    "America/Mexico_City",
    "America/Regina",
    "America/Bogota",
    "America/New_York",
    "America/Indiana/Indianapolis",
    "America/Caracas",
    "America/Asuncion",
    "America/Halifax",
    "America/Cuiaba",
    "America/La_Paz",
    "America/Santiago",
    "America/St_Johns",
    "America/Sao_Paulo",
    "America/Argentina/Buenos_Aires",
    "America/Cayenne",
    "America/Godthab",
    "America/Montevideo",
    "America/Bahia",
    "Etc/GMT+2",
    "Atlantic/Azores",
    "Atlantic/Cape_Verde",
    "Africa/Casablanca",
    "Etc/GMT",
    "Europe/London",
    "Atlantic/Reykjavik",
    "Europe/Berlin",
    "Europe/Budapest",
    "Europe/Paris",
    "Europe/Warsaw",
    "Africa/Lagos",
    "Africa/Windhoek",
    "Europe/Bucharest",
    "Asia/Beirut",
    "Africa/Cairo",
    "Asia/Damascus",
    "Africa/Johannesburg",
    "Europe/Kyiv",
    "Europe/Istanbul",
    "Asia/Jerusalem",
    "Asia/Amman",
    "Asia/Baghdad",
    "Europe/Kaliningrad",
    "Asia/Riyadh",
    "Africa/Nairobi",
    "Asia/Tehran",
    "Asia/Dubai",
    "Asia/Baku",
    "Europe/Moscow",
    "Indian/Mauritius",
    "Asia/Tbilisi",
    "Asia/Yerevan",
    "Asia/Kabul",
    "Asia/Karachi",
    "Asia/Toshkent",
    "Asia/Tashkent",
    "Asia/Kolkata",
    "Asia/Colombo",
    "Asia/Kathmandu",
    "Asia/Astana",
    "Asia/Almaty",
    "Asia/Dhaka",
    "Asia/Yekaterinburg",
    "Asia/Yangon",
    "Asia/Rangoon",
    "Asia/Bangkok",
    "Asia/Novosibirsk",
    "Asia/Shanghai",
    "Asia/Krasnoyarsk",
    "Asia/Singapore",
    "Australia/Perth",
    "Asia/Taipei",
    "Asia/Ulaanbaatar",
    "Asia/Irkutsk",
    "Asia/Tokyo",
    "Asia/Seoul",
    "Australia/Adelaide",
    "Australia/Darwin",
    "Australia/Brisbane",
    "Australia/Sydney",
    "Pacific/Port_Moresby",
    "Australia/Hobart",
    "Asia/Yakutsk",
    "Pacific/Guadalcanal",
    "Asia/Vladivostok",
    "Pacific/Auckland",
    "Etc/GMT-12",
    "Pacific/Fiji",
    "Asia/Magadan",
    "Pacific/Tongatapu",
    "Pacific/Apia",
    "Pacific/Kiritimati",
];

/// The zone a name stands for, when it is a name the reference says a time can be told in: the
/// one Windows has for a zone, written as Windows writes it, or one of [`ADDITIONAL_ZONES`]. Any
/// other name of the IANA database (`Europe/Amsterdam`) is not one the service is documented to
/// take, and neither is an offset or a name Windows once had.
fn documented_zone(zone: &str) -> Option<mailcraft_time::Zone> {
    if zone.len() > 64 {
        return None;
    }
    let known = mailcraft_time::Zone::named(zone)?;
    (known.windows_name() == Some(zone) || ADDITIONAL_ZONES.contains(&zone)).then_some(known)
}

/// How far east of UTC a zone is at an instant (minutes), for the zones the stand-in knows:
/// UTC, a few with daylight saving by the rules of the United States and of Europe as they are
/// written out here (so that they are not this program's own tables that say when it is), and
/// every other name the reference lists ([`documented_zone`]).
pub fn zone_offset(zone: &str, instant: Timestamp) -> Option<i32> {
    #[derive(Clone, Copy)]
    enum Rule {
        None,
        Us,
        Europe,
    }
    let (standard, rule) = match zone {
        "UTC" | "Etc/UTC" | "Etc/GMT" | "tzone://Microsoft/Utc" => (0, Rule::None),
        "Eastern Standard Time" | "America/New_York" => (-300, Rule::Us),
        "Central Standard Time" | "America/Chicago" => (-360, Rule::Us),
        "Mountain Standard Time" | "America/Denver" => (-420, Rule::Us),
        "Pacific Standard Time" | "America/Los_Angeles" => (-480, Rule::Us),
        "GMT Standard Time" | "Europe/London" => (0, Rule::Europe),
        "W. Europe Standard Time" | "Europe/Berlin" => (60, Rule::Europe),
        "India Standard Time" | "Asia/Kolkata" => (330, Rule::None),
        "Tokyo Standard Time" | "Asia/Tokyo" => (540, Rule::None),
        _ => return documented_zone(zone).map(|known| known.offset_at(instant)),
    };
    let year = DateTime::from_timestamp(instant, 0).date.year;
    // The `n`th Sunday of a month (the last one for 0), at midnight UTC.
    let sunday = |month: u8, n: i64| -> Option<Timestamp> {
        let first = Date::new(year, month, 1)?;
        let to_sunday = (i64::from(Weekday::Sun.index()) - i64::from(first.weekday().index())).rem_euclid(7);
        let day = if n > 0 {
            first.add_days(to_sunday + 7 * (n - 1))
        } else {
            let last = first.end_of_month();
            last.add_days(-(i64::from(last.weekday().index()) - i64::from(Weekday::Sun.index())).rem_euclid(7))
        };
        Some(DateTime::at_midnight(day).to_timestamp(0))
    };
    let summer = match rule {
        Rule::None => None,
        // From two in the morning standard time to two in the morning daylight time.
        Rule::Us => {
            sunday(3, 2).zip(sunday(11, 1)).map(|(from, to)| (from + 7_200 - i64::from(standard) * 60, to + 7_200 - i64::from(standard + 60) * 60))
        }
        // From one in the morning UTC to one in the morning UTC.
        Rule::Europe => sunday(3, 0).zip(sunday(10, 0)).map(|(from, to)| (from + 3_600, to + 3_600)),
    };
    Some(if summer.is_some_and(|(from, to)| (from..to).contains(&instant)) { standard + 60 } else { standard })
}

/// The instant of a wall-clock time in a zone the stand-in knows.
fn instant_in(wall: DateTime, zone: &str) -> Option<Timestamp> {
    let guess = wall.to_timestamp(zone_offset(zone, wall.to_timestamp(0))?);
    Some(wall.to_timestamp(zone_offset(zone, guess)?))
}

/// The wall-clock time of an instant in a zone the stand-in knows (UTC for one it doesn't).
fn wall_in(instant: Timestamp, zone: &str) -> DateTime {
    DateTime::from_timestamp(instant, zone_offset(zone, instant).unwrap_or(0))
}

/// `2026-10-15T16:00:00` or `2026-10-15T16:00:00.0000000`: a date and a time, and no zone.
fn wall_of(text: &str) -> Option<DateTime> {
    let (date, time) = text.split_once('T')?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let digits = |part: &str, n: usize| part.len() == n && part.bytes().all(|b| b.is_ascii_digit());
    let mut parts = clock.split(':');
    let (hour, minute, second) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !digits(hour, 2) || !digits(minute, 2) || !digits(second, 2) {
        return None;
    }
    if fraction.is_empty() || fraction.len() > 7 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if date.len() != 10 {
        return None;
    }
    DateTime::new(Date::parse_iso(date)?, hour.parse().ok()?, minute.parse().ok()?, second.parse().ok()?)
}

/// A piece of a path with its `%XX` undone (a `+` is a plus here, not a space).
fn piece(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: Option<&u8>| b.and_then(|b| char::from(*b).to_digit(16)).and_then(|d| u8::try_from(d).ok());
    let (mut out, mut i) = (Vec::with_capacity(bytes.len()), 0);
    while let Some(&b) = bytes.get(i) {
        match (b, hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) {
            (b'%', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `2026-10-12T13:00:00.0000000`, as Graph writes a time.
fn stamp(instant: Timestamp) -> String {
    format!("{}.0000000", DateTime::from_timestamp(instant, 0).iso())
}

fn instant_text(instant: Timestamp) -> String {
    format!("{}Z", DateTime::from_timestamp(instant, 0).iso())
}

impl Stored {
    /// The zone its end was written in: the one of its start unless it was told in another.
    pub fn end_zone(&self) -> &str {
        if self.ends_in.is_empty() { &self.zone } else { &self.ends_in }
    }

    fn change_key(&self) -> String {
        format!("CQAAABYAAAAiIsqMbYjsT5e/T7KzowPTAAA{:05}+A==", self.version)
    }

    fn kind(&self) -> &'static str {
        match (self.recurrence.is_some(), self.series.is_empty()) {
            (true, _) => "seriesMaster",
            (false, false) => "exception",
            (false, true) => "singleInstance",
        }
    }

    /// How the series repeats, as this crate reads a recurrence, with its first day.
    fn rule(&self) -> Option<(mailcraft_model::Recurrence, Date)> {
        self.recurrence.as_ref().and_then(super::rule_of)
    }

    /// When the series has its occurrences between `from` and `to` (those it still has).
    fn occurrences(&self, from: Timestamp, to: Timestamp) -> Vec<Timestamp> {
        let Some((rule, first)) = self.rule() else { return Vec::new() };
        let at = wall_in(self.start, &self.zone);
        let day = |instant: Timestamp| DateTime::from_timestamp(instant.clamp(0, 4_000_000_000), 0).date;
        let length = self.end.saturating_sub(self.start);
        rule.dates(first, day(from).add_days(-2), day(to).add_days(2), &[])
            .into_iter()
            .filter_map(|on| instant_in(DateTime { date: on, ..at }, &self.zone))
            .filter(|start| *start < to && start.saturating_add(length.max(1)) > from && !self.gone.contains(start))
            .take(MAX_OCCURRENCES)
            .collect()
    }

    /// The id of the occurrence that the series has at `original`.
    fn occurrence_id(&self, original: Timestamp) -> String {
        format!("{}_{original:011}BBA=", self.id.trim_end_matches('='))
    }

    /// The occurrence of this series at `original`, as an event of its own.
    fn occurrence(&self, original: Timestamp) -> Stored {
        Stored {
            id: self.occurrence_id(original),
            start: original,
            end: original.saturating_add(self.end.saturating_sub(self.start)),
            recurrence: None,
            series: self.id.clone(),
            original,
            gone: Vec::new(),
            ..self.clone()
        }
    }
}

/// What an answer is asked to look like.
#[derive(Clone, Debug, Default)]
struct Look {
    /// The body as text rather than HTML (`Prefer: outlook.body-content-type="text"`).
    text: bool,
    /// The properties asked for by name; all of them when none were.
    select: Option<Vec<String>>,
    /// It is one occurrence of a series as the series has it.
    occurrence: bool,
    /// The view it is for can't be asked for these by name, and leaves them out.
    no_stamps: bool,
}

impl State {
    fn new_id(&mut self) -> String {
        self.made += 1;
        format!(
            "AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAAAAAAiQ8W967B7TKBjgx9rVEURBwAiIsqMbYjsT5e-T7KzowPTAAAAAAENAAAiIsq/MbYjs+T5e_T7Kzow{:05}AAA=",
            self.made
        )
    }

    /// A calendar of the stand-in's making, beside the main one: its id.
    pub fn add_calendar(&mut self, name: &str, can_edit: bool, shared: bool) -> String {
        self.made += 1;
        let id = format!(
            "AQMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAADIkPFveuwe0ygY4Mfa1RFEQcAIiLKjG2I7E_Xvk-ys6MD0wAAAgEG{:04}/+A==",
            self.made
        );
        let online = vec!["teamsForBusiness".to_string()];
        self.calendars.push(Calendar { id: id.clone(), name: name.to_string(), main: false, can_edit, shared, online });
        id
    }

    /// The link to join by, for an event that was just made an online meeting.
    fn join_link(&mut self, event: &mut Stored) {
        if event.online && event.join_url.is_empty() {
            self.made += 1;
            event.join_url = format!("https://teams.example.org/l/meetup-join/19%3ameeting_{:05}%40thread.v2/0", self.made);
        }
    }

    /// An event that is in the calendar already, an hour long, the owner's own: its id.
    pub fn add_event(&mut self, calendar: &str, subject: &str, start: Timestamp) -> String {
        let id = self.new_id();
        self.clock += 1;
        self.events.push(Stored {
            id: id.clone(),
            calendar: calendar.to_string(),
            version: 1,
            uid: format!("040000008200E00074C5B7101A82E008{:032X}", self.made),
            subject: subject.to_string(),
            start,
            end: start + 3_600,
            zone: "UTC".into(),
            show_as: "busy".into(),
            sensitivity: "normal".into(),
            is_organizer: true,
            response: "organizer".into(),
            modified: self.clock,
            ..Default::default()
        });
        id
    }

    /// A folder of mail of the stand-in's making, in the folder `parent` (`None`: at the top of
    /// the mailbox): its id, as Graph writes ids (with `-`, `_` and `=`).
    pub fn add_folder(&mut self, name: &str, parent: Option<&str>) -> String {
        self.made += 1;
        let id = format!("AQMkAGV4YW1wbGUtb3JnLW1haWxib3gtZm9sZGVycwAuAAADk1v_Qx7RkS-nTtqB3L0d9EBAFh2c-Kp_0aTmWvN4uYx1RoAAAIB{:04}AAAA=", self.made);
        let parent = parent.map(str::to_string).unwrap_or_else(|| self.well_known("msgfolderroot"));
        self.folders.push(MailFolder { id: id.clone(), name: name.to_string(), parent, ..Default::default() });
        id
    }

    /// Another mailbox at `address`, with the top and the special folders every mailbox has and
    /// no mail (#70); `full_access` says whether the owner of the token may open it.
    pub fn add_other(&mut self, address: &str, full_access: bool) {
        let owners = std::mem::take(&mut self.folders);
        self.made += 1;
        let top = MailFolder {
            id: format!("AQMkAGV4YW1wbGUtb3JnLXNoYXJlZC1tYWlsYm94AC4AAAOTW-9DHtGRL6dO2oHcvR30QEAWHZz4qn_RpOZa83i5jHVGgAACAQ{:04}AAAA=", self.made),
            name: "Top of Information Store".into(),
            parent: "AQMkAGV4YW1wbGUtb3JnLXNoYXJlZC1tYWlsYm94AC4AAAOTW-9DHtGRL6dO2oHcvR30QEAWHZz4qn_RpOZa83i5jHVGgAACAQEAAAA=".into(),
            well_known: "msgfolderroot".into(),
            ..Default::default()
        };
        self.folders.push(top);
        for (name, well_known) in SPECIAL_FOLDERS {
            let id = self.add_folder(name, None);
            if let Some(folder) = self.folders.iter_mut().find(|folder| folder.id == id) {
                folder.well_known = well_known.into();
            }
        }
        let folders = std::mem::replace(&mut self.folders, owners);
        self.others.push(OtherMailbox {
            address: address.to_string(),
            full_access,
            refusing: None,
            send_as_denied: false,
            folders,
            mailbox: Mail::default(),
        });
    }

    /// The other mailbox at `address` (#70), to set up and to look at.
    pub fn other(&mut self, address: &str) -> Option<&mut OtherMailbox> {
        self.others.iter_mut().find(|other| other.address.eq_ignore_ascii_case(address))
    }

    /// Whose mailbox the links it writes name: `me`, or `users/{address}` (#70).
    fn whose(&self) -> &str {
        if self.at.is_empty() { "me" } else { &self.at }
    }

    /// `/v1.0/users/{user}/…`: another mailbox, answered as the owner's own is, with its folders
    /// and its mail in their place for as long as it takes (#70).
    fn of_user(&mut self, asked: &Asked, method: &str, user: &str, rest: &[&str]) -> Result<Answer, Refusal> {
        let Some(at) = self.others.iter().position(|other| other.address.eq_ignore_ascii_case(user)) else {
            return Err(refuse(404, "ErrorInvalidUser", format!("The requested user '{user}' is invalid.")));
        };
        let Some(other) = self.others.get_mut(at) else { return Err(not_found()) };
        if let Some((status, code)) = other.refusing {
            return Err(refuse(status, code, "Refused, as asked."));
        }
        if !other.full_access {
            return Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again."));
        }
        let sending = method == "POST" && matches!(rest, ["sendMail"] | ["messages", _, "send"]);
        if sending && other.send_as_denied {
            return Err(refuse(
                403,
                "ErrorSendAsDenied",
                "The user account which was used to submit this request does not have the right to send mail on behalf of the specified sending account.",
            ));
        }
        let (mut folders, mut mailbox) = (std::mem::take(&mut other.folders), std::mem::take(&mut other.mailbox));
        std::mem::swap(&mut self.folders, &mut folders);
        std::mem::swap(&mut self.mailbox, &mut mailbox);
        // Its drafts are its own, from its own address (#70, slice 4).
        let owner = std::mem::replace(&mut self.owner, user.to_string());
        let (local, domain) = user.split_once('@').unwrap_or((user, ""));
        self.at = format!("users/{}@{}", crate::http::encode(local), crate::http::encode(domain));
        let answer = match rest {
            ["mailFolders"] => self.mail_folders(asked, method, None, &[]),
            ["mailFolders", folder, rest @ ..] => self.mail_folders(asked, method, Some(folder), rest),
            ["messages", id, "$value"] if method == "GET" => self.message_value(id),
            ["messages"] if method == "GET" && asked.param("$search").is_some() => self.search_messages(asked, None),
            // Its drafts made, changed and sent, and its mail sent, there alone: the copy in its
            // own Sent Items (#70, slice 4). (A large attachment's upload is the owner's only.)
            ["messages"] if method == "POST" => self.create_message(asked),
            ["sendMail"] if method == "POST" => self.send_mail(asked),
            ["messages", id, "send"] if method == "POST" => self.send_draft(asked, id),
            ["messages", id, action @ ("createReply" | "createReplyAll" | "createForward")] if method == "POST" => {
                self.create_answer(asked, id, action)
            }
            ["messages", id, "attachments"] => self.attachments_at(asked, method, id, None),
            ["messages", id, "attachments", attachment] if *attachment != "createUploadSession" => {
                self.attachments_at(asked, method, id, Some(attachment))
            }
            // Its messages read, changed, moved, copied and deleted, within it alone: a folder
            // of another mailbox is no destination here (#70, slice 3).
            ["messages", id] => self.message_at(asked, method, id, None),
            ["messages", id, action @ ("move" | "copy")] => self.message_at(asked, method, id, Some(action)),
            [first, ..] => Err(refuse(400, "BadRequest", format!("Resource not found for the segment '{first}'."))),
            [] => Err(refuse(405, "MethodNotAllowed", "The method is not allowed on this resource.")),
        };
        self.at.clear();
        self.owner = owner;
        std::mem::swap(&mut self.folders, &mut folders);
        std::mem::swap(&mut self.mailbox, &mut mailbox);
        if let Some(other) = self.others.get_mut(at) {
            (other.folders, other.mailbox) = (folders, mailbox);
        }
        answer
    }

    /// The id of the folder that answers to a well-known name (`inbox`, `msgfolderroot`…);
    /// empty when the mailbox has none.
    pub fn well_known(&self, name: &str) -> String {
        self.folders.iter().find(|folder| folder.well_known == name).map(|folder| folder.id.clone()).unwrap_or_default()
    }

    /// A folder's id as the cached configuration of Outlook on the web writes it: the same id
    /// in the other alphabet (`/` for `-`, `+` for `_`).
    pub fn configuration_id(id: &str) -> String {
        id.replace('-', "/").replace('_', "+")
    }

    /// Put the folder `id` among the Favorites, at the place `index`, under the name it has now.
    pub fn add_favorite(&mut self, id: &str, index: i64) {
        let name = self.folders.iter().find(|folder| folder.id == id).map(|folder| folder.name.clone()).unwrap_or_default();
        let entry = Favorite { name, index: Some(index), kind: "Folder".into(), folder_id: State::configuration_id(id) };
        self.favorites.get_or_insert_with(Vec::new).push(entry);
    }

    /// The document the cached configuration of Outlook on the web holds: a dictionary of which
    /// one entry is the list of Favorites (when there is one), among others that are nobody's
    /// business ([`CONFIGURATION_CANARY`] stands for all of that).
    pub fn configuration_xml(&self) -> String {
        let (a, b) = (self.prefixes.0.as_str(), self.prefixes.1.as_str());
        let escaped = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let storage = "http://schemas.datacontract.org/2004/07/Microsoft.Exchange.Data.Storage";
        // (The pair's own element has a name with a generated end to it: nothing to go by.)
        let pair = |key: &str, kind: &str, value: &str| {
            format!(
                "<{a}:KeyValueOfstringSerializableDataBaseQx7_Zr2Lm><{a}:Key>{key}</{a}:Key>\
                 <{a}:Value i:type=\"{b}:{kind}\" xmlns:{b}=\"{storage}.Configuration\">{value}</{a}:Value>\
                 </{a}:KeyValueOfstringSerializableDataBaseQx7_Zr2Lm>\n"
            )
        };
        let mut parts = pair("OWA.ThemeData", "ThemeData", &format!("<{b}:value>{CONFIGURATION_CANARY}</{b}:value>"));
        if let Some(favorites) = &self.favorites {
            let mut list = String::new();
            for (n, favorite) in favorites.iter().enumerate() {
                let index = favorite.index.map(|index| format!("<{b}:Index>{index}</{b}:Index>")).unwrap_or_default();
                let setting =
                    |key: &str, value: &str| format!("<{b}:KeyValueType><{b}:Key>{key}</{b}:Key><{b}:Value>{value}</{b}:Value></{b}:KeyValueType>");
                let settings = if favorite.folder_id.is_empty() {
                    String::new()
                } else {
                    format!("{}{}", setting("FolderId", &escaped(&favorite.folder_id)), setting("ChangeKey", "AQAAABYAAAB4"))
                };
                list.push_str(&format!(
                    "<{b}:OutlookFavorite><{b}:Client>{}</{b}:Client><{b}:DisplayName>{}</{b}:DisplayName>\
                     <{b}:Id>RgAAAABleGFtcGxlLWZhdm9yaXRl{n:04}</{b}:Id>{index}\
                     <{b}:LastModifiedTime>2026-03-17T09:00:00Z</{b}:LastModifiedTime>\
                     <{b}:SingleValueSettings>{settings}</{b}:SingleValueSettings><{b}:Type>{}</{b}:Type></{b}:OutlookFavorite>\n",
                    if n == 0 { "OWA" } else { "Legacy" },
                    escaped(&favorite.name),
                    escaped(&favorite.kind),
                ));
            }
            parts.push_str(&pair("OWA.OutlookFavorites", "OutlookFavoritesData", &format!("<{b}:value>{list}</{b}:value>")));
        }
        parts.push_str(&pair(".docx", "FileTypeData", &format!("<{b}:value>{CONFIGURATION_CANARY}</{b}:value>")));
        format!(
            "<AggregatedUserConfiguration.MementoClass xmlns=\"{storage}\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\">\
             <ConcurrentUpdates/><FailedToLoad/>\
             <Parts xmlns:{a}=\"http://schemas.microsoft.com/2003/10/Serialization/Arrays\">\n{parts}</Parts>\
             </AggregatedUserConfiguration.MementoClass>"
        )
    }

    /// The folder of mail an address names: by its id, or by a well-known name.
    fn mail_folder(&self, named: &str) -> Result<&MailFolder, Refusal> {
        if let Some(folder) = self.folders.iter().find(|folder| !folder.well_known.is_empty() && folder.well_known == named) {
            return Ok(folder);
        }
        // What can't be an id at all is told apart from one that names nothing. (An id is
        // written in an address in Graph's own alphabet: with `-` and `_`, never `/` or `+`.)
        if named.len() < 20 || named.chars().any(|c| !(c.is_ascii_alphanumeric() || "-_=".contains(c))) {
            return Err(refuse(400, "ErrorInvalidIdMalformed", "Id is malformed."));
        }
        self.folders.iter().find(|folder| folder.id == named).ok_or_else(not_found)
    }

    /// `GET /me/mailFolders/{id}`: the folder, with the properties that were asked for by name.
    fn mail_folder_read(&mut self, asked: &Asked, named: &str) -> Result<Answer, Refusal> {
        if let Some((other, _)) = asked.query.iter().find(|(name, _)| name != "$select") {
            return Err(bad(format!("The query parameter '{other}' is not one this stand-in answers for a mail folder.")));
        }
        let select: Option<Vec<&str>> = asked.param("$select").map(|names| names.split(',').map(str::trim).collect());
        if let Some(unknown) = select.iter().flatten().find(|name| !MAIL_FOLDER_PROPERTIES.contains(name)) {
            return Err(refuse(400, "BadRequest", format!("Could not find a property named '{unknown}' on type 'microsoft.graph.mailFolder'.")));
        }
        let folder = self.mail_folder(named)?.clone();
        let mut all = self.folder_json(&folder, select.as_deref());
        all["@odata.context"] = json!(format!("{}/v1.0/$metadata#users('me')/mailFolders/$entity", self.base));
        Ok((200, Some(all)))
    }

    /// A folder as Graph writes one, with only the properties `select` names (and its id) when
    /// it names any.
    fn folder_json(&self, folder: &MailFolder, select: Option<&[&str]>) -> Value {
        let children = self.folders.iter().filter(|other| other.parent == folder.id).count();
        let (total, unread) = self.mailbox.counts(&folder.id);
        let mut all = json!({
            "id": folder.id,
            "displayName": folder.name,
            "parentFolderId": folder.parent,
            "childFolderCount": children,
            "unreadItemCount": unread,
            "totalItemCount": total,
            "sizeInBytes": 0,
            "isHidden": folder.hidden,
        });
        if let (Some(select), Some(map)) = (select, all.as_object_mut()) {
            map.retain(|name, _| name == "id" || select.contains(&name.as_str()));
        }
        if let (true, Some(map)) = (self.uncounted, all.as_object_mut()) {
            map.retain(|name, _| name != "totalItemCount" && name != "unreadItemCount");
        }
        if folder.search {
            all["@odata.type"] = json!("#microsoft.graph.mailSearchFolder");
        }
        all
    }

    /// Everything under `/v1.0/me/mailFolders`: a list of folders, one folder, and what is done to
    /// one. Every one of them may be refused (403) or told to come back later (429).
    fn mail_folders(&mut self, asked: &Asked, method: &str, folder: Option<&str>, rest: &[&str]) -> Result<Answer, Refusal> {
        if self.folders_denied {
            return Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again."));
        }
        if self.throttle_folders > 0 {
            self.throttle_folders -= 1;
            return Err(refuse(
                429,
                "ApplicationThrottled",
                "Application is over its MailboxConcurrency limit while the request was being processed.",
            ));
        }
        let root = self.well_known("msgfolderroot");
        match (method, folder, rest) {
            ("GET", None, []) => self.folder_list(asked, &root),
            ("POST", None, []) => self.folder_create(asked, &root),
            ("GET", Some(named), []) => self.mail_folder_read(asked, named),
            ("PATCH", Some(named), []) => self.folder_rename(asked, named),
            ("DELETE", Some(named), []) => self.folder_delete(named),
            ("GET", Some(named), ["childFolders"]) => {
                let id = self.mail_folder(named)?.id.clone();
                self.folder_list(asked, &id)
            }
            ("POST", Some(named), ["childFolders"]) => {
                let id = self.mail_folder(named)?.id.clone();
                self.folder_create(asked, &id)
            }
            ("POST", Some(named), ["move"]) => self.folder_move(asked, named),
            ("GET", Some(named), ["messages", "delta"]) => self.message_delta(asked, named),
            ("GET", Some(named), ["messages"]) if asked.param("$search").is_some() => self.search_messages(asked, Some(named)),
            ("GET", Some(named), ["messages"]) => self.folder_messages(asked, named),
            ("POST", Some(named), ["messages"]) => self.create_in_folder(asked, named),
            (_, Some(named), ["messages", id, action @ ..]) if action.len() <= 1 => {
                self.mail_folder(named)?;
                self.message_at(asked, method, id, action.first().copied())
            }
            (_, _, [] | ["childFolders"] | ["move"]) => Err(refuse(405, "MethodNotAllowed", "The method is not allowed on this resource.")),
            (_, _, [other, ..]) => Err(refuse(400, "BadRequest", format!("Resource not found for the segment '{other}'."))),
        }
    }

    /// The folders that lie in `parent`, in pages: hidden ones only when they are asked for.
    fn folder_list(&self, asked: &Asked, parent: &str) -> Result<Answer, Refusal> {
        if self.broken_lists.iter().any(|broken| broken == parent) {
            return Err(refuse(500, "ErrorInternalServerError", "An internal server error occurred. The operation failed."));
        }
        if let Some((other, _)) = asked.query.iter().find(|(name, _)| !["$select", "$top", "$skip", "includeHiddenFolders"].contains(&name.as_str()))
        {
            return Err(bad(format!("The query parameter '{other}' is not one this stand-in answers for a list of folders.")));
        }
        let select: Option<Vec<&str>> = asked.param("$select").map(|names| names.split(',').map(str::trim).collect());
        if let Some(unknown) = select.iter().flatten().find(|name| !MAIL_FOLDER_PROPERTIES.contains(name)) {
            return Err(refuse(400, "BadRequest", format!("Could not find a property named '{unknown}' on type 'microsoft.graph.mailFolder'.")));
        }
        let hidden = self.lists_hidden || asked.param("includeHiddenFolders") == Some("true");
        let entries: Vec<Value> = self
            .folders
            .iter()
            .filter(|folder| folder.parent == parent && (hidden || !folder.hidden))
            .map(|folder| self.folder_json(folder, select.as_deref()))
            .collect();
        let number = |name: &str| asked.param(name).map(|n| n.parse::<usize>().map_err(|_| bad(format!("The value of '{name}' is not a number."))));
        let top = number("$top").transpose()?.unwrap_or(PAGE);
        if !(1..=1_000).contains(&top) {
            return Err(bad("The value of '$top' must be between 1 and 1000."));
        }
        let top = if self.folder_page > 0 { top.min(self.folder_page) } else { top };
        let skip = number("$skip").transpose()?.unwrap_or(0);
        let total = entries.len();
        let page: Vec<Value> = entries.into_iter().skip(skip).take(top).collect();
        let mut list = json!({"@odata.context": format!("{}/v1.0/$metadata#users('me')/mailFolders", self.base), "value": page});
        if skip.saturating_add(top) < total {
            let path = asked.target.split('?').next().unwrap_or("");
            let mut query: Vec<(String, String)> = asked.query.iter().filter(|(name, _)| name != "$skip" && name != "$top").cloned().collect();
            query.push(("$top".into(), top.to_string()));
            query.push(("$skip".into(), (skip + top).to_string()));
            let pairs: Vec<(&str, &str)> = query.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            list["@odata.nextLink"] = json!(format!("{}{path}?{}", self.base, crate::http::form(&pairs)));
        }
        Ok((200, Some(list)))
    }

    /// Whether `parent` has a folder called `name` (in any case) other than `except`.
    fn name_taken(&self, parent: &str, name: &str, except: &str) -> bool {
        self.folders.iter().any(|folder| folder.parent == parent && folder.id != except && folder.name.to_lowercase() == name.to_lowercase())
    }

    /// The `displayName` of a body that may say nothing else (`isHidden` too, when `hidden` may).
    fn display_name(asked: &Asked, hidden: bool) -> Result<String, Refusal> {
        let body = asked.json();
        let Some(map) = body.as_object() else { return Err(bad("The body of the request is not a JSON object.")) };
        if let Some(other) = map.keys().find(|key| *key != "displayName" && !(hidden && *key == "isHidden")) {
            return Err(bad(format!("The property '{other}' does not exist on type 'microsoft.graph.mailFolder' or can't be written.")));
        }
        match map.get("displayName").and_then(Value::as_str) {
            Some(name) if !name.trim().is_empty() => Ok(name.to_string()),
            _ => Err(bad("A folder needs a displayName.")),
        }
    }

    /// `POST …/childFolders` (or `POST /me/mailFolders`, at the top): a new folder, 201.
    fn folder_create(&mut self, asked: &Asked, parent: &str) -> Result<Answer, Refusal> {
        let name = State::display_name(asked, true)?;
        if self.name_taken(parent, &name, "") {
            return Err(refuse(409, "ErrorFolderExists", "A folder with the specified name already exists."));
        }
        let hidden = asked.json().get("isHidden").and_then(Value::as_bool).unwrap_or(false);
        let id = self.add_folder(&name, Some(parent));
        let folder = self.folders.iter_mut().find(|folder| folder.id == id).ok_or_else(not_found)?;
        folder.hidden = hidden;
        let folder = folder.clone();
        Ok((201, Some(self.folder_json(&folder, None))))
    }

    /// `PATCH /me/mailFolders/{id}`: its name, 200. The special folders keep theirs.
    fn folder_rename(&mut self, asked: &Asked, named: &str) -> Result<Answer, Refusal> {
        let name = State::display_name(asked, false)?;
        let folder = self.mail_folder(named)?.clone();
        if !folder.well_known.is_empty() {
            return Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again."));
        }
        if self.name_taken(&folder.parent, &name, &folder.id) {
            return Err(refuse(409, "ErrorFolderExists", "A folder with the specified name already exists."));
        }
        self.rename_folder(&folder.id, &name);
        let folder = self.mail_folder(&folder.id)?.clone();
        Ok((200, Some(self.folder_json(&folder, None))))
    }

    /// `POST /me/mailFolders/{id}/move` with a `destinationId` (an id or a well-known name): 201.
    fn folder_move(&mut self, asked: &Asked, named: &str) -> Result<Answer, Refusal> {
        let body = asked.json();
        let keys: Vec<&String> = body.as_object().map(|map| map.keys().collect()).unwrap_or_default();
        let Some(to) = body.get("destinationId").and_then(Value::as_str).filter(|_| keys.len() == 1) else {
            return Err(bad("A move takes a destinationId and nothing else."));
        };
        let folder = self.mail_folder(named)?.clone();
        let destination = self.mail_folder(to)?.id.clone();
        if !folder.well_known.is_empty() {
            return Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again."));
        }
        if self.under(&destination, &folder.id) {
            return Err(bad("A folder can't be moved into itself."));
        }
        if self.name_taken(&destination, &folder.name, &folder.id) {
            return Err(refuse(409, "ErrorFolderExists", "A folder with the specified name already exists."));
        }
        self.move_folder(&folder.id, &destination);
        let folder = self.mail_folder(&folder.id)?.clone();
        Ok((201, Some(self.folder_json(&folder, None))))
    }

    /// `DELETE /me/mailFolders/{id}`: into Deleted Items; from there (or under it), gone with
    /// everything under it. 204.
    fn folder_delete(&mut self, named: &str) -> Result<Answer, Refusal> {
        let folder = self.mail_folder(named)?.clone();
        if !folder.well_known.is_empty() {
            return Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again."));
        }
        let deleted = self.well_known("deleteditems");
        if self.under(&folder.id, &deleted) {
            self.remove_folder(&folder.id);
        } else {
            self.move_folder(&folder.id, &deleted);
        }
        Ok((204, None))
    }

    /// Whether the folder `id` is `ancestor` or lies somewhere under it.
    pub fn under(&self, id: &str, ancestor: &str) -> bool {
        let mut at = id.to_string();
        for _ in 0..64 {
            if at == ancestor {
                return true;
            }
            match self.folders.iter().find(|folder| folder.id == at) {
                Some(folder) => at = folder.parent.clone(),
                None => return false,
            }
        }
        false
    }

    /// The folder called `name`, wherever it is.
    pub fn folder_named(&self, name: &str) -> Option<&MailFolder> {
        self.folders.iter().find(|folder| folder.name == name)
    }

    /// Rename the folder `id` (as another program would).
    pub fn rename_folder(&mut self, id: &str, name: &str) {
        if let Some(folder) = self.folders.iter_mut().find(|folder| folder.id == id) {
            folder.name = name.to_string();
        }
    }

    /// Put the folder `id` in `parent` (as another program would).
    pub fn move_folder(&mut self, id: &str, parent: &str) {
        if let Some(folder) = self.folders.iter_mut().find(|folder| folder.id == id) {
            folder.parent = parent.to_string();
        }
    }

    /// Take the folder `id` away with everything under it (as another program would).
    pub fn remove_folder(&mut self, id: &str) {
        let gone: Vec<String> = self.folders.iter().filter(|folder| self.under(&folder.id, id)).map(|folder| folder.id.clone()).collect();
        self.folders.retain(|folder| !gone.contains(&folder.id));
    }

    /// `/beta/me/mailFolders/{folder}/userConfigurations…`: the cached configuration of Outlook
    /// on the web, which is in the root of the mailbox and is only ever read here.
    fn user_configuration(&self, asked: &Asked, method: &str, rest: &[&str]) -> Result<Answer, Refusal> {
        match rest {
            // There is no list of them: an item is asked for by its name.
            [_, "userConfigurations"] => Err(refuse(400, "ErrorInvalidRequest", "The OData request is not supported.")),
            [folder, "userConfigurations", name] => {
                if self.configuration == Configuration::Denied {
                    return Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again."));
                }
                // (Reading is all the permission is for.)
                if method != "GET" {
                    return Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again."));
                }
                if !asked.query.is_empty() {
                    return Err(bad("This method doesn't support OData query parameters."));
                }
                if self.configuration == Configuration::Missing || *folder != "root" || *name != "Aggregated.OwaUserConfiguration" {
                    return Err(not_found());
                }
                use base64::Engine as _;
                let document = self.configuration_document.clone().unwrap_or_else(|| self.configuration_xml());
                let context = format!("{}/beta/$metadata#users('me')/mailFolders('root')/userConfigurations/$entity", self.base);
                let packed = base64::engine::general_purpose::STANDARD.encode(document);
                Ok((200, Some(json!({"@odata.context": context, "id": name, "xmlData": packed}))))
            }
            [first, ..] => Err(refuse(400, "BadRequest", format!("Resource not found for the segment '{first}'."))),
            [] => Err(refuse(405, "MethodNotAllowed", "The method is not allowed on this resource.")),
        }
    }

    /// The main calendar's id.
    pub fn main_calendar(&self) -> String {
        self.calendars.iter().find(|calendar| calendar.main).map(|calendar| calendar.id.clone()).unwrap_or_default()
    }

    /// The event called `subject`, when there is one.
    pub fn event(&self, subject: &str) -> Option<&Stored> {
        self.events.iter().find(|event| event.subject == subject)
    }

    /// The requests that changed something: their methods and the pieces of their paths.
    pub fn writes(&self) -> Vec<(String, Vec<String>)> {
        self.requests
            .iter()
            .filter(|asked| asked.method != "GET" && asked.path.last().is_none_or(|last| last != "$batch"))
            .map(|asked| (asked.method.clone(), asked.path.clone()))
            .collect()
    }

    fn calendar(&self, id: &str) -> Option<&Calendar> {
        self.calendars.iter().find(|calendar| calendar.id == id)
    }

    /// The calendar an address names (`None`: the events of the account, which is every
    /// calendar of its own), when there is one.
    fn calendar_of(&self, named: Option<&str>) -> Result<Option<&Calendar>, Refusal> {
        match named {
            None => Ok(None),
            Some(id) => {
                self.calendar(id).map(Some).ok_or_else(|| refuse(404, "ErrorItemNotFound", "The specified folder could not be found in the store."))
            }
        }
    }

    /// Where the event `id` is among the events, as an address through `calendar` finds it.
    fn find(&self, calendar: Option<&str>, id: &str) -> Result<usize, Refusal> {
        // What can't be an id at all is told apart from one that names nothing.
        if id.len() < 20 || id.chars().any(|c| !(c.is_ascii_alphanumeric() || "-_=/+".contains(c))) {
            return Err(refuse(400, "ErrorInvalidIdMalformed", "Id is malformed."));
        }
        let at = self.events.iter().position(|event| event.id == id).ok_or_else(not_found)?;
        let home = self.events.get(at).map(|event| event.calendar.clone()).unwrap_or_default();
        match calendar {
            Some(calendar) if calendar != home => Err(not_found()),
            // The copy of a calendar that somebody else shares is found through it only.
            None if self.calendar(&home).is_some_and(|calendar| calendar.shared) => Err(not_found()),
            _ => Ok(at),
        }
    }

    /// The series and the time that an occurrence's id names.
    fn occurrence_named(&self, id: &str) -> Option<(usize, Timestamp)> {
        let (series, rest) = id.rsplit_once('_')?;
        let original = rest.strip_suffix("BBA=")?.parse::<Timestamp>().ok()?;
        let at = self.events.iter().position(|event| event.recurrence.is_some() && event.id.trim_end_matches('=') == series)?;
        let master = self.events.get(at)?;
        master.occurrences(original, original.saturating_add(1)).contains(&original).then_some((at, original))
    }

    /// One more version of the calendar: what a view that lags behind is answered from.
    fn changing(&mut self) {
        self.before = Some(self.events.clone());
        self.clock += 1;
    }

    fn json_of(&self, event: &Stored, look: &Look) -> Value {
        let person = |address: &str| json!({"emailAddress": {"name": address.split('@').next().unwrap_or(""), "address": address}});
        let time = |instant: Timestamp| json!({"dateTime": stamp(instant), "timeZone": "UTC"});
        let body = if look.text {
            json!({"contentType": "text", "content": event.body})
        } else {
            let escaped = event.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "<br>");
            json!({"contentType": "html", "content": format!("<html><head></head><body>{escaped}</body></html>")})
        };
        let attendees: Vec<Value> = event
            .attendees
            .iter()
            .map(|attendee| {
                json!({
                    "type": attendee.get("type").cloned().unwrap_or(json!("required")),
                    "status": {"response": "none", "time": "0001-01-01T00:00:00Z"},
                    "emailAddress": attendee.get("emailAddress").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        let kind = if look.occurrence { "occurrence" } else { event.kind() };
        // A range that names no zone of its own is in the zone of the event, and says so.
        let mut recurrence = event.recurrence.clone().unwrap_or(Value::Null);
        if let Some(range) = recurrence.get_mut("range").and_then(Value::as_object_mut)
            && !range.contains_key("recurrenceTimeZone")
        {
            range.insert("recurrenceTimeZone".into(), json!(event.zone));
        }
        let mut all = json!({
            "@odata.etag": format!("W/\"{}\"", event.change_key()),
            "id": event.id,
            "createdDateTime": instant_text(event.modified),
            "lastModifiedDateTime": instant_text(event.modified),
            "changeKey": event.change_key(),
            "categories": event.categories,
            "originalStartTimeZone": event.zone,
            "originalEndTimeZone": event.end_zone(),
            "iCalUId": event.uid,
            "reminderMinutesBeforeStart": event.reminder_minutes,
            "isReminderOn": event.reminder_on,
            "hasAttachments": false,
            "subject": event.subject,
            "bodyPreview": event.body.chars().take(255).collect::<String>(),
            "importance": if event.importance.is_empty() { "normal" } else { event.importance.as_str() },
            "sensitivity": event.sensitivity,
            "isAllDay": event.all_day,
            "isCancelled": false,
            "isOrganizer": event.is_organizer,
            "responseRequested": !event.no_response_requested,
            "seriesMasterId": if event.series.is_empty() { Value::Null } else { json!(event.series) },
            "showAs": event.show_as,
            "type": kind,
            "webLink": "https://outlook.example/calendar/item",
            "onlineMeetingUrl": Value::Null,
            "isOnlineMeeting": event.online,
            "responseStatus": {"response": event.response, "time": "0001-01-01T00:00:00Z"},
            "body": body,
            "start": time(event.start),
            "end": time(event.end),
            "location": {"displayName": event.location, "locationType": "default", "uniqueIdType": "unknown", "address": {}, "coordinates": {}},
            "locations": [],
            "recurrence": recurrence,
            "attendees": attendees,
            "organizer": person(if event.is_organizer { &self.owner } else { "bo@example.org" }),
        });
        let Some(map) = all.as_object_mut() else { return all };
        map.insert("allowNewTimeProposals".into(), json!(!event.no_new_times));
        map.insert("onlineMeetingProvider".into(), json!(if event.online { event.online_provider.as_str() } else { "unknown" }));
        map.insert("onlineMeeting".into(), if event.online { json!({"joinUrl": event.join_url}) } else { Value::Null });
        // Only returned when a client set it; and when a series has it only for what is one of
        // its days.
        if !event.transaction_id.is_empty() {
            map.insert("transactionId".into(), json!(event.transaction_id));
        }
        if !event.series.is_empty() {
            map.insert("originalStart".into(), json!(instant_text(event.original)));
        }
        if look.no_stamps && look.select.is_some() {
            map.remove("createdDateTime");
            map.remove("lastModifiedDateTime");
        }
        if let Some(select) = &look.select {
            map.retain(|name, _| name == "@odata.etag" || name == "id" || select.iter().any(|asked| asked == name));
        }
        all
    }

    /// What a body says of when an event is, checked against what Graph takes: the instants of
    /// its start and its end, whether it is a whole day, and the zones its start and its end are
    /// told in. `now` is the event as it is, for a change that doesn't say everything.
    fn times_of(body: &Map<String, Value>, now: Option<&Stored>) -> Result<(Timestamp, Timestamp, bool, String, String), Refusal> {
        let invalid = |message: &str| refuse(400, "ErrorPropertyValidationFailure", message.to_string());
        let read = |name: &str| -> Result<Option<(DateTime, String)>, Refusal> {
            let Some(value) = body.get(name) else { return Ok(None) };
            let Some(said) = value.as_object() else { return Err(bad(format!("The property '{name}' is a dateTimeTimeZone."))) };
            if let Some(other) = said.keys().find(|key| !matches!(key.as_str(), "dateTime" | "timeZone")) {
                return Err(bad(format!("The property '{other}' does not exist on type 'microsoft.graph.dateTimeTimeZone'.")));
            }
            let text = said.get("dateTime").and_then(Value::as_str).ok_or_else(|| bad(format!("'{name}' has no dateTime.")))?;
            let zone = said.get("timeZone").and_then(Value::as_str).ok_or_else(|| bad(format!("'{name}' has no timeZone.")))?;
            let wall = wall_of(text).ok_or_else(|| bad(format!("The value '{text}' of '{name}' is not a date and a time.")))?;
            if zone_offset(zone, 0).is_none() {
                let message = format!("A valid TimeZone value must be specified. The following TimeZone value is not supported: '{zone}'.");
                return Err(refuse(400, "TimeZoneNotSupportedException", message));
            }
            Ok(Some((wall, zone.to_string())))
        };
        let (start, end) = (read("start")?, read("end")?);
        let all_day = match body.get("isAllDay") {
            None => now.is_some_and(|now| now.all_day),
            Some(Value::Bool(all_day)) => *all_day,
            Some(_) => return Err(bad("The property 'isAllDay' is a Boolean.")),
        };
        let kept = |instant: Timestamp, zone: &str| (wall_in(instant, zone), zone.to_string());
        let start = match (start, now) {
            (Some(start), _) => start,
            (None, Some(now)) => kept(now.start, &now.zone),
            (None, None) => return Err(invalid("The Start property is required.")),
        };
        let end = match (end, now) {
            (Some(end), _) => end,
            (None, Some(now)) => kept(now.end, now.end_zone()),
            (None, None) => return Err(invalid("The End property is required.")),
        };
        let instants = instant_in(start.0, &start.1).zip(instant_in(end.0, &end.1));
        let Some((from, to)) = instants else { return Err(invalid("The time is out of range.")) };
        if to < from {
            return Err(refuse(400, "ErrorCalendarEndDateIsEarlierThanStartDate", "The end date is earlier than the start date."));
        }
        if all_day {
            if start.1 != end.1 {
                return Err(invalid("The start and the end of an all-day event must be in the same time zone."));
            }
            if start.0.seconds_of_day() != 0 {
                return Err(invalid("The Event.Start property for an all-day event needs to be set to midnight."));
            }
            if end.0.seconds_of_day() != 0 {
                return Err(invalid("The Event.End property for an all-day event needs to be set to midnight."));
            }
            if start.0.date.days_until(end.0.date) < 1 {
                return Err(invalid("The duration of an all-day event must be at least 24 hours."));
            }
        }
        Ok((from, to, all_day, start.1, end.1))
    }

    /// A recurrence as Graph takes one, for an event that starts on `first` (the day of its
    /// start as its start says it).
    fn recurrence_of(value: &Value, first: Date) -> Result<Option<Value>, Refusal> {
        if value.is_null() {
            return Ok(None);
        }
        let invalid = |message: String| refuse(400, "ErrorPropertyValidationFailure", message);
        let (Some(pattern), Some(range)) = (value.get("pattern").and_then(Value::as_object), value.get("range").and_then(Value::as_object)) else {
            return Err(bad("A recurrence has a pattern and a range."));
        };
        let known = |of: &Map<String, Value>, names: &[&str], what: &str| match of.keys().find(|key| !names.contains(&key.as_str())) {
            Some(other) => Err(bad(format!("The property '{other}' does not exist on type 'microsoft.graph.{what}'."))),
            None => Ok(()),
        };
        known(pattern, &["type", "interval", "month", "dayOfMonth", "daysOfWeek", "firstDayOfWeek", "index"], "recurrencePattern")?;
        known(range, &["type", "startDate", "endDate", "recurrenceTimeZone", "numberOfOccurrences"], "recurrenceRange")?;
        let number = |of: &Map<String, Value>, name: &str| of.get(name).and_then(Value::as_u64);
        let day_name = |day: &Value| day.as_str().is_some_and(|day| DAY_NAMES.contains(&day.to_ascii_lowercase().as_str()));
        let days = pattern.get("daysOfWeek").and_then(Value::as_array).is_some_and(|days| !days.is_empty() && days.iter().all(day_name));
        if !number(pattern, "interval").is_some_and(|interval| (1..=999).contains(&interval)) {
            return Err(invalid("The recurrence pattern's interval is required and must be 1 or more.".into()));
        }
        let kind = pattern.get("type").and_then(Value::as_str).unwrap_or("");
        let day_of_month = number(pattern, "dayOfMonth").is_some_and(|day| (1..=31).contains(&day));
        let month = number(pattern, "month").is_some_and(|month| (1..=12).contains(&month));
        let complete = match kind {
            "daily" => true,
            "weekly" | "relativeMonthly" => days,
            "absoluteMonthly" => day_of_month,
            "absoluteYearly" => day_of_month && month,
            "relativeYearly" => days && month,
            other => return Err(invalid(format!("The recurrence pattern type '{other}' is not supported."))),
        };
        if !complete {
            return Err(invalid(format!("The recurrence pattern of type '{kind}' is missing a property it requires.")));
        }
        if pattern.get("firstDayOfWeek").is_some_and(|day| !day_name(day)) {
            return Err(invalid("The recurrence pattern's firstDayOfWeek is not a day of the week.".into()));
        }
        let index = pattern.get("index").map(|index| index.as_str().unwrap_or(""));
        if index.is_some_and(|index| !["first", "second", "third", "fourth", "last"].contains(&index)) {
            return Err(invalid("The recurrence pattern's index is not one of first, second, third, fourth, last.".into()));
        }
        let date = |name: &str| range.get(name).and_then(Value::as_str).and_then(Date::parse_iso);
        let Some(starts) = date("startDate") else { return Err(invalid("The recurrence range's startDate is required.".into())) };
        if starts != first {
            return Err(invalid(format!(
                "The recurrence range's startDate ({}) must be the date of the event's start ({}).",
                starts.iso(),
                first.iso()
            )));
        }
        match range.get("type").and_then(Value::as_str).unwrap_or("") {
            "noEnd" => {}
            "endDate" if date("endDate").is_some_and(|ends| ends >= starts) => {}
            "endDate" => return Err(invalid("The recurrence range's endDate is required and must not be before its startDate.".into())),
            "numbered" if number(range, "numberOfOccurrences").is_some_and(|n| n >= 1) => {}
            "numbered" => return Err(invalid("The recurrence range's numberOfOccurrences must be 1 or more.".into())),
            other => return Err(invalid(format!("The recurrence range type '{other}' is not supported."))),
        }
        if let Some(zone) = range.get("recurrenceTimeZone")
            && !zone.as_str().is_some_and(|zone| zone_offset(zone, 0).is_some())
        {
            return Err(refuse(400, "TimeZoneNotSupportedException", "The recurrenceTimeZone is not supported."));
        }
        Ok(Some(value.clone()))
    }

    /// Put what a body says into `event`, when it is what Graph takes. Whether the people
    /// invited would hear of it.
    fn write(&self, event: &mut Stored, body: &Value, making: bool) -> Result<bool, Refusal> {
        let Some(body) = body.as_object() else { return Err(refuse(400, "BadRequest", "Unable to read JSON request payload.")) };
        for name in body.keys() {
            // (Whether another time may be proposed is said when an event is made: it is not
            // among what the reference says an update may change.)
            let writable = WRITABLE.contains(&name.as_str())
                || (making && matches!(name.as_str(), "transactionId" | "allowNewTimeProposals"))
                || (self.updates_new_times && name == "allowNewTimeProposals");
            if name == "transactionId" && !making {
                return Err(refuse(400, "ErrorInvalidPropertyUpdate", "The transactionId of an event can't be changed in an update."));
            }
            if !writable && PROPERTIES.contains(&name.as_str()) {
                return Err(refuse(400, "ErrorInvalidPropertySet", format!("The property '{name}' is read-only.")));
            }
            if !writable {
                let message = format!("The property '{name}' does not exist on type 'Microsoft.OutlookServices.Event'.");
                return Err(refuse(400, "RequestBodyRead", message));
            }
        }
        let text = |name: &str| -> Result<Option<String>, Refusal> {
            match body.get(name) {
                None => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(_) => Err(bad(format!("The property '{name}' is a String."))),
            }
        };
        let one_of = |name: &str, values: &[&str]| -> Result<Option<String>, Refusal> {
            match text(name)? {
                Some(value) if values.contains(&value.as_str()) => Ok(Some(value)),
                Some(value) => Err(bad(format!("The value '{value}' is not one that '{name}' can have."))),
                None => Ok(None),
            }
        };
        let flag = |name: &str| -> Result<Option<bool>, Refusal> {
            match body.get(name) {
                None => Ok(None),
                Some(Value::Bool(on)) => Ok(Some(*on)),
                Some(_) => Err(bad(format!("The property '{name}' is a Boolean."))),
            }
        };
        let mut seen = false;
        if let Some(subject) = text("subject")? {
            seen |= subject != event.subject;
            event.subject = subject;
        }
        if let Some(said) = body.get("body") {
            let kind = said.get("contentType").and_then(Value::as_str).map(str::to_ascii_lowercase);
            let (Some("text" | "html"), Some(content)) = (kind.as_deref(), said.get("content").and_then(Value::as_str)) else {
                return Err(bad("The property 'body' is an itemBody: a contentType (text or html) and a content."));
            };
            seen |= content != event.body;
            event.body = content.to_string();
        }
        if let Some(place) = body.get("location") {
            let Some(name) = place.get("displayName").and_then(Value::as_str) else { return Err(bad("The property 'location' has a displayName.")) };
            seen |= name != event.location;
            event.location = name.to_string();
        }
        if body.contains_key("start") || body.contains_key("end") || body.contains_key("isAllDay") || making {
            let (start, end, all_day, zone, ends_in) = State::times_of(body, (!making).then_some(&*event))?;
            seen |= (start, end, all_day) != (event.start, event.end, event.all_day);
            // (A server that says yes and keeps the time it had: what no client may take for done.)
            // The zones a time was told in are the event's from then on, the instant the same or not.
            if !self.keeps_times || making {
                (event.start, event.end, event.all_day) = (start, end, all_day);
                event.ends_in = if ends_in == zone { String::new() } else { ends_in };
                event.zone = zone;
            }
        }
        if let Some(show_as) = one_of("showAs", &["free", "tentative", "busy", "oof", "workingElsewhere", "unknown"])? {
            event.show_as = show_as;
        }
        if let Some(sensitivity) = one_of("sensitivity", &["normal", "personal", "private", "confidential"])? {
            seen |= sensitivity != event.sensitivity;
            event.sensitivity = sensitivity;
        }
        if let Some(importance) = one_of("importance", &["low", "normal", "high"])? {
            let importance = if importance == "normal" { String::new() } else { importance };
            seen |= importance != event.importance;
            event.importance = importance;
        }
        if let Some(asked) = flag("responseRequested")? {
            seen |= asked == event.no_response_requested;
            event.no_response_requested = !asked;
        }
        if let Some(allowed) = flag("allowNewTimeProposals")? {
            seen |= !making && allowed == event.no_new_times;
            event.no_new_times = !allowed;
        }
        let wants_online = flag("isOnlineMeeting")?;
        let provider = one_of("onlineMeetingProvider", &["unknown", "teamsForBusiness", "skypeForBusiness", "skypeForConsumer"])?;
        let named = provider.filter(|provider| provider != "unknown");
        // Once it is an online meeting it stays one, held with what it is held with: whatever
        // is said about that afterwards is ignored.
        if !event.online && (wants_online == Some(true) || named.is_some()) {
            let allowed = self.calendar(&event.calendar).map(|calendar| calendar.online.clone()).unwrap_or_default();
            match named.or_else(|| allowed.first().cloned()).filter(|with| allowed.contains(with)) {
                Some(with) => {
                    (event.online, event.online_provider) = (true, with);
                    seen = true;
                }
                None if self.drops_online => {}
                None => return Err(bad("The online meeting provider is not one that this calendar allows.")),
            }
        }
        match body.get("isReminderOn") {
            None => {}
            Some(Value::Bool(on)) => event.reminder_on = *on,
            Some(_) => return Err(bad("The property 'isReminderOn' is a Boolean.")),
        }
        match body.get("reminderMinutesBeforeStart") {
            None => {}
            Some(minutes) => {
                event.reminder_minutes = minutes.as_u64().ok_or_else(|| bad("The property 'reminderMinutesBeforeStart' is an Int32."))?
            }
        }
        if let Some(categories) = body.get("categories") {
            let names = categories.as_array().filter(|names| names.iter().all(Value::is_string));
            let Some(names) = names else { return Err(bad("The property 'categories' is a collection of String.")) };
            event.categories = names.iter().filter_map(Value::as_str).map(str::to_string).collect();
        }
        if let Some(people) = body.get("attendees") {
            let invited = |attendee: &Value| {
                let address = attendee.get("emailAddress").and_then(|address| address.get("address")).and_then(Value::as_str);
                let kind = attendee.get("type").and_then(Value::as_str).unwrap_or("required");
                address.is_some_and(|address| address.contains('@')) && ["required", "optional", "resource"].contains(&kind)
            };
            let Some(people) = people.as_array().filter(|people| people.iter().all(invited)) else {
                return Err(bad("The property 'attendees' is a collection of attendee: each an emailAddress with an address, and a type."));
            };
            seen |= *people != event.attendees;
            event.attendees = people.clone();
        }
        if let Some(recurrence) = body.get("recurrence") {
            let first = wall_in(event.start, &event.zone).date;
            let rule = State::recurrence_of(recurrence, first)?;
            seen |= rule != event.recurrence;
            event.recurrence = rule;
            if event.recurrence.is_none() {
                event.gone.clear();
            }
        }
        if let Some(mark) = text("transactionId")? {
            event.transaction_id = mark;
        }
        Ok(seen)
    }

    /// The events of a view between `from` and `to` in `calendar` (`None`: the main one), as
    /// they are now or as they were before the last change.
    fn view(&mut self, calendar: Option<&str>, from: Timestamp, to: Timestamp, lagging: bool) -> Vec<(Stored, bool)> {
        let home = calendar.map(str::to_string).unwrap_or_else(|| self.main_calendar());
        let stale = if lagging && self.stale_views > 0 && self.before.is_some() {
            self.stale_views -= 1;
            self.before.clone()
        } else {
            None
        };
        let events = stale.as_ref().unwrap_or(&self.events);
        let mut out: Vec<(Stored, bool)> = Vec::new();
        for event in events.iter().filter(|event| event.calendar == home) {
            if event.recurrence.is_some() {
                out.extend(event.occurrences(from, to).into_iter().map(|at| (event.occurrence(at), true)));
            } else if event.start < to && (event.end > from || event.start >= from) {
                out.push((event.clone(), false));
            }
        }
        out.sort_by_key(|(event, _)| (event.start, event.id.clone()));
        out
    }

    /// A page of `entries` for a list that was asked for with `asked`.
    fn page(&self, asked: &Asked, entries: Vec<Value>) -> Result<Answer, Refusal> {
        let number = |name: &str| asked.param(name).map(|n| n.parse::<usize>().map_err(|_| bad(format!("The value of '{name}' is not a number."))));
        let top = number("$top").transpose()?;
        if top.is_some_and(|top| !(1..=1_000).contains(&top)) {
            return Err(bad("The value of '$top' must be between 1 and 1000."));
        }
        let (top, skip) = (top.unwrap_or(PAGE), number("$skip").transpose()?.unwrap_or(0));
        let total = entries.len();
        let page: Vec<Value> = entries.into_iter().skip(skip).take(top).collect();
        let mut list = json!({"@odata.context": format!("{}/v1.0/$metadata#users('me')/events", self.base), "value": page});
        if skip.saturating_add(top) < total {
            let path = asked.target.split('?').next().unwrap_or("");
            let mut query: Vec<(String, String)> = asked.query.iter().filter(|(name, _)| name != "$skip").cloned().collect();
            query.push(("$skip".into(), (skip + top).to_string()));
            let pairs: Vec<(&str, &str)> = query.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            list["@odata.nextLink"] = json!(format!("{}{path}?{}", self.base, crate::http::form(&pairs)));
        }
        Ok((200, Some(list)))
    }

    /// How an answer to `asked` is to look.
    fn look(asked: &Asked) -> Result<Look, Refusal> {
        let prefer = asked.header("prefer").to_ascii_lowercase();
        let select = match asked.param("$select") {
            None => None,
            Some(names) => {
                let names: Vec<String> = names.split(',').map(|name| name.trim().to_string()).collect();
                if let Some(unknown) = names.iter().find(|name| !PROPERTIES.contains(&name.as_str())) {
                    return Err(refuse(400, "BadRequest", format!("Could not find a property named '{unknown}' on type 'microsoft.graph.event'.")));
                }
                Some(names)
            }
        };
        Ok(Look { text: prefer.contains("outlook.body-content-type=\"text\""), select, occurrence: false, no_stamps: false })
    }

    /// An instant in a query (`2026-10-08T19:00:00Z`, with an offset, or with none: UTC).
    fn window(asked: &Asked) -> Result<(Timestamp, Timestamp), Refusal> {
        let needs = || {
            let message = "This request requires a time window specified by the query string parameters StartDateTime and EndDateTime.";
            refuse(400, "ErrorInvalidParameter", message)
        };
        let read = |name: &str| asked.param(name).and_then(|text| mailcraft_time::iso_to_timestamp(text, 0)).ok_or_else(needs);
        let (from, to) = (read("startDateTime")?, read("endDateTime")?);
        if to < from {
            return Err(bad("The end of the time window is before its start."));
        }
        Ok((from, to))
    }

    /// May `calendar` be written to?
    fn writable(&self, calendar: &str) -> Result<(), Refusal> {
        match self.calendar(calendar) {
            Some(calendar) if calendar.can_edit => Ok(()),
            Some(_) => Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again.")),
            None => Err(not_found()),
        }
    }

    /// Answer one request.
    fn answer(&mut self, asked: &Asked) -> Result<Answer, Refusal> {
        let answer = self.answer_of(asked);
        if answer.is_ok()
            && let Some(at) = self.lose_next.iter().position(|pattern| drafts::aims_at(pattern, &asked.method, &asked.path))
        {
            self.lose_next.remove(at);
            self.lose_answer = true;
        }
        answer
    }

    fn answer_of(&mut self, asked: &Asked) -> Result<Answer, Refusal> {
        // An upload's own address: its token is its permission (no Authorization).
        if let [first, token] = asked.path.as_slice()
            && first == "upload"
        {
            return self.upload_piece(asked, token);
        }
        if asked.header("authorization") != format!("Bearer {}", self.token) {
            return Err(refuse(401, "InvalidAuthenticationToken", "Access token validation failure."));
        }
        let aimed = format!("{} /{}", asked.method, asked.path.join("/"));
        if let Some(at) = self.fail_next.iter().position(|(method, _, _)| *method == asked.method || *method == aimed) {
            let (_, status, code) = self.fail_next.remove(at);
            let code: &'static str = match code.as_str() {
                "TooManyRequests" => "TooManyRequests",
                "ErrorItemNotFound" => "ErrorItemNotFound",
                "ErrorAccessDenied" => "ErrorAccessDenied",
                "ErrorInvalidRequest" => "ErrorInvalidRequest",
                "ErrorInvalidUser" => "ErrorInvalidUser",
                "ErrorQuotaExceeded" => "ErrorQuotaExceeded",
                "ErrorSendAsDenied" => "ErrorSendAsDenied",
                _ => "ServiceUnavailable",
            };
            return Err(refuse(status, code, "The stand-in was told to say so."));
        }
        // A message is also made, or sent, from its MIME in base64.
        // (Another mailbox's, at `/users/{address}/…`, as the owner's: #70.)
        let made_or_sent = |what: &String| what == "messages" || what == "sendMail";
        let mime = asked.method == "POST"
            && match asked.path.as_slice() {
                [v, me, what] => v == "v1.0" && me == "me" && made_or_sent(what),
                [v, users, _, what] => v == "v1.0" && users == "users" && made_or_sent(what),
                _ => false,
            }
            && asked.header("content-type").to_ascii_lowercase().starts_with("text/plain");
        if !mime && !asked.body.is_empty() && !asked.header("content-type").to_ascii_lowercase().starts_with("application/json") {
            return Err(refuse(415, "RequestBodyRead", "A body is JSON, and says so in its Content-Type."));
        }
        let path: Vec<&str> = asked.path.iter().map(String::as_str).collect();
        let method = asked.method.as_str();
        match (method, path.as_slice()) {
            ("POST", ["v1.0", "$batch"]) => self.batch(asked),
            // (User configurations are in the beta version only.)
            (_, ["beta", "me", "mailFolders", rest @ ..]) => self.user_configuration(asked, method, rest),
            (_, ["v1.0", "me", "mailFolders"]) => self.mail_folders(asked, method, None, &[]),
            (_, ["v1.0", "me", "mailFolders", folder, rest @ ..]) => self.mail_folders(asked, method, Some(folder), rest),
            (_, ["v1.0", "me", rest @ ..]) => self.me(asked, method, rest),
            (_, ["v1.0", "users", user, rest @ ..]) => {
                let user = user.to_string();
                self.of_user(asked, method, &user, rest)
            }
            (_, [first, ..]) => Err(refuse(400, "BadRequest", format!("Resource not found for the segment '{first}'."))),
            _ => Err(refuse(400, "BadRequest", "Resource not found.")),
        }
    }

    fn me(&mut self, asked: &Asked, method: &str, rest: &[&str]) -> Result<Answer, Refusal> {
        let no_way = || refuse(405, "MethodNotAllowed", "The method is not allowed on this resource.");
        match rest {
            ["events", tail @ ..] => self.events_at(asked, method, None, tail),
            ["calendar", "events", tail @ ..] => {
                let main = self.main_calendar();
                self.events_at(asked, method, Some(main.as_str()), tail)
            }
            ["calendars", calendar, "events", tail @ ..] => self.events_at(asked, method, Some(*calendar), tail),
            ["calendarView"] if method == "GET" => self.calendar_view(asked, None),
            ["calendars", calendar, "calendarView"] if method == "GET" => self.calendar_view(asked, Some(*calendar)),
            ["calendars"] | ["calendarGroups", _, "calendars"] if method == "GET" => {
                let list = self
                    .calendars
                    .iter()
                    .map(|calendar| {
                        json!({
                            "id": calendar.id, "name": calendar.name, "color": "auto", "hexColor": "", "canEdit": calendar.can_edit,
                            "canShare": !calendar.shared, "isDefaultCalendar": calendar.main,
                            "allowedOnlineMeetingProviders": calendar.online,
                            "defaultOnlineMeetingProvider": calendar.online.first().map_or("unknown", String::as_str),
                            "owner": {"name": if calendar.shared { "Somebody Else" } else { "Owner" }, "address": if calendar.shared { "else@example.org" } else { self.owner.as_str() }},
                        })
                    })
                    .collect();
                self.page(asked, list)
            }
            ["calendarGroups"] if method == "GET" => {
                let group = json!({"id": "AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAAAAAAiQ8W967B7TKBjgx9rVEURBgAA-group=", "name": "My Calendars"});
                self.page(asked, vec![group])
            }
            ["messages"] if method == "POST" => self.create_message(asked),
            ["messages"] if method == "GET" && asked.param("$search").is_some() => self.search_messages(asked, None),
            ["sendMail"] if method == "POST" => self.send_mail(asked),
            ["messages", id, "send"] if method == "POST" => self.send_draft(asked, id),
            ["messages", id, "$value"] if method == "GET" => self.message_value(id),
            ["messages", id, action @ ("createReply" | "createReplyAll" | "createForward")] if method == "POST" => {
                self.create_answer(asked, id, action)
            }
            ["messages", id, "attachments"] => self.attachments_at(asked, method, id, None),
            ["messages", id, "attachments", "createUploadSession"] if method == "POST" => self.upload_session(asked, id),
            ["messages", id, "attachments", attachment] => self.attachments_at(asked, method, id, Some(attachment)),
            ["messages", id] => self.message_at(asked, method, id, None),
            ["messages", id, action] => self.message_at(asked, method, id, Some(action)),
            ["contacts"] | ["outlook", "masterCategories"] if method == "GET" => self.page(asked, Vec::new()),
            ["mailboxSettings", "workingHours"] if method == "GET" => Ok((
                200,
                Some(json!({
                    "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
                    "startTime": "08:00:00.0000000", "endTime": "17:00:00.0000000", "timeZone": {"name": "UTC"},
                })),
            )),
            ["calendarView"] | ["calendars"] | ["calendarGroups"] | ["contacts"] => Err(no_way()),
            [first, ..] => Err(refuse(400, "BadRequest", format!("Resource not found for the segment '{first}'."))),
            [] => Err(no_way()),
        }
    }

    fn calendar_view(&mut self, asked: &Asked, calendar: Option<&str>) -> Result<Answer, Refusal> {
        self.calendar_of(calendar)?;
        let (from, to) = State::window(asked)?;
        let look = Look { no_stamps: true, ..State::look(asked)? };
        // Only the first page of a view lags behind: a list is one answer, however many pages.
        let lagging = asked.param("$skip").is_none();
        let entries = self.view(calendar, from, to, lagging);
        let list = entries.iter().map(|(event, occurrence)| self.json_of(event, &Look { occurrence: *occurrence, ..look.clone() })).collect();
        self.page(asked, list)
    }

    /// `/events…` of the account (`calendar` is `None`) or of one of its calendars.
    fn events_at(&mut self, asked: &Asked, method: &str, calendar: Option<&str>, tail: &[&str]) -> Result<Answer, Refusal> {
        self.calendar_of(calendar)?;
        let look = State::look(asked)?;
        match (method, tail) {
            ("POST", []) => {
                let home = calendar.map(str::to_string).unwrap_or_else(|| self.main_calendar());
                self.writable(&home)?;
                let body = asked.json();
                // The same mark again: the event that was made with it, and not a second one.
                let mark = body.get("transactionId").and_then(Value::as_str).unwrap_or("");
                if let Some(made) = self.events.iter().find(|event| !mark.is_empty() && event.transaction_id == mark) {
                    return Ok((201, Some(self.json_of(made, &look))));
                }
                let mut event = Stored {
                    calendar: home,
                    version: 1,
                    show_as: "busy".into(),
                    sensitivity: "normal".into(),
                    zone: "UTC".into(),
                    is_organizer: true,
                    response: "organizer".into(),
                    reminder_on: true,
                    reminder_minutes: 15,
                    ..Default::default()
                };
                self.write(&mut event, &body, true)?;
                self.changing();
                event.id = self.new_id();
                event.uid = format!("040000008200E00074C5B7101A82E008{:032X}", self.made);
                event.modified = self.clock;
                self.join_link(&mut event);
                if !event.attendees.is_empty() {
                    self.mail.push(format!("invitation:{}:{}", event.subject, event.attendees.len()));
                }
                let made = self.json_of(&event, &look);
                self.events.push(event);
                Ok((201, Some(made)))
            }
            ("GET", [id]) => match self.find(calendar, id) {
                Ok(at) => Ok((200, self.events.get(at).map(|event| self.json_of(event, &look)))),
                // One occurrence of a series, as the series has it.
                Err(why) => match self.occurrence_named(id) {
                    Some((series, original)) => {
                        let one = self.events.get(series).map(|series| series.occurrence(original));
                        Ok((200, one.map(|one| self.json_of(&one, &Look { occurrence: true, ..look }))))
                    }
                    None => Err(why),
                },
            },
            ("GET", [id, "instances"]) => {
                let at = self.find(calendar, id)?;
                let (from, to) = State::window(asked)?;
                let Some(series) = self.events.get(at).filter(|event| event.recurrence.is_some()).cloned() else {
                    return Err(bad("The event is not a series: it has no instances."));
                };
                let mut all: Vec<(Stored, bool)> = series.occurrences(from, to).into_iter().map(|at| (series.occurrence(at), true)).collect();
                let within = |event: &&Stored| event.series == series.id && event.start < to && event.start >= from;
                all.extend(self.events.iter().filter(within).map(|event| (event.clone(), false)));
                all.sort_by_key(|(event, _)| event.start);
                let list = all.iter().map(|(event, occurrence)| self.json_of(event, &Look { occurrence: *occurrence, ..look.clone() })).collect();
                self.page(asked, list)
            }
            ("PATCH", [id]) => {
                let body = asked.json();
                let at = match self.find(calendar, id) {
                    Ok(at) => at,
                    // A change to one occurrence makes it an exception, under the id it had.
                    Err(why) => {
                        let Some((series, original)) = self.occurrence_named(id) else { return Err(why) };
                        let Some(master) = self.events.get(series).cloned() else { return Err(why) };
                        self.writable(&master.calendar)?;
                        let mut one = master.occurrence(original);
                        self.write(&mut one, &body, false)?;
                        // It may not cross the occurrences beside it.
                        let beside = master.occurrences(original.saturating_sub(400 * DAY), original.saturating_add(400 * DAY));
                        let crosses = |other: &Timestamp| (*other < original && one.start <= *other) || (*other > original && one.start >= *other);
                        if beside.iter().any(crosses) {
                            let message = "Modified occurrence is crossing or overlapping adjacent occurrence.";
                            return Err(refuse(400, "ErrorOccurrenceCrossingBoundary", message));
                        }
                        self.changing();
                        one.version += 1;
                        one.modified = self.clock;
                        self.join_link(&mut one);
                        if let Some(master) = self.events.get_mut(series) {
                            master.gone.push(original);
                        }
                        let made = self.json_of(&one, &look);
                        self.events.push(one);
                        return Ok((200, Some(made)));
                    }
                };
                let Some(mut event) = self.events.get(at).cloned() else { return Err(not_found()) };
                self.writable(&event.calendar)?;
                let seen = self.write(&mut event, &body, false)?;
                self.changing();
                event.version += 1;
                event.modified = self.clock;
                self.join_link(&mut event);
                if seen && event.is_organizer && !event.attendees.is_empty() {
                    self.mail.push(format!("update:{}:{}", event.subject, event.attendees.len()));
                }
                let changed = self.json_of(&event, &look);
                if let Some(slot) = self.events.get_mut(at) {
                    *slot = event;
                }
                Ok((200, Some(changed)))
            }
            ("DELETE", [id]) => {
                match self.find(calendar, id) {
                    Ok(at) => {
                        let Some(event) = self.events.get(at).cloned() else { return Err(not_found()) };
                        self.writable(&event.calendar)?;
                        self.changing();
                        if event.is_organizer && !event.attendees.is_empty() {
                            self.mail.push(format!("cancellation:{}:{}:", event.subject, event.attendees.len()));
                        }
                        self.events.retain(|other| other.id != event.id && other.series != event.id);
                    }
                    Err(why) => {
                        let Some((series, original)) = self.occurrence_named(id) else { return Err(why) };
                        let home = self.events.get(series).map(|series| series.calendar.clone()).unwrap_or_default();
                        self.writable(&home)?;
                        self.changing();
                        if let Some(master) = self.events.get_mut(series) {
                            master.gone.push(original);
                        }
                    }
                }
                Ok((204, None))
            }
            ("POST", [id, action @ ("cancel" | "accept" | "tentativelyAccept" | "decline")]) => {
                let body = asked.json();
                let comment = body.get("comment").and_then(Value::as_str).unwrap_or("").to_string();
                // (Another time is proposed with a tentative answer or with a decline, never
                // with a yes.)
                let allowed: &[&str] = match *action {
                    "cancel" => &["comment"],
                    "accept" => &["comment", "sendResponse"],
                    _ => &["comment", "sendResponse", "proposedNewTime"],
                };
                if let Some(other) = body.as_object().and_then(|body| body.keys().find(|key| !allowed.contains(&key.as_str()))) {
                    return Err(bad(format!("The parameter '{other}' is not one of this action's.")));
                }
                match body.get("sendResponse") {
                    None | Some(Value::Bool(_)) => {}
                    Some(_) => return Err(bad("The parameter 'sendResponse' is a Boolean.")),
                }
                // A timeSlot: a start and an end, each a date and a time in a zone.
                let proposed = match body.get("proposedNewTime") {
                    None => None,
                    Some(slot) => {
                        let Some(slot) = slot.as_object() else { return Err(bad("The parameter 'proposedNewTime' is a timeSlot.")) };
                        if let Some(other) = slot.keys().find(|key| !matches!(key.as_str(), "start" | "end")) {
                            return Err(bad(format!("The property '{other}' does not exist on type 'microsoft.graph.timeSlot'.")));
                        }
                        let (from, to, ..) = State::times_of(slot, None)?;
                        Some((from, to))
                    }
                };
                let at = self.find(calendar, id)?;
                let Some(event) = self.events.get(at).cloned() else { return Err(not_found()) };
                self.writable(&event.calendar)?;
                match (*action, event.is_organizer) {
                    ("cancel", false) => {
                        let message = "Your request can't be completed. You need to be an organizer to cancel a meeting.";
                        return Err(refuse(400, "ErrorAccessDenied", message));
                    }
                    ("cancel", true) => {
                        self.changing();
                        self.mail.push(format!("cancellation:{}:{}:{comment}", event.subject, event.attendees.len()));
                        self.events.retain(|other| other.id != event.id && other.series != event.id);
                    }
                    (_, true) => return Err(bad("Your request can't be completed. You can't respond to a meeting you organize.")),
                    (answer, false) => {
                        let send = body.get("sendResponse").and_then(Value::as_bool).unwrap_or(true);
                        if proposed.is_some() && event.no_new_times {
                            return Err(bad("The organizer of this event doesn't allow new time proposals."));
                        }
                        if proposed.is_some() && !send {
                            return Err(bad("A new time is proposed with a response that is sent: sendResponse can't be false."));
                        }
                        self.changing();
                        if send {
                            self.mail.push(format!("response:{}:{answer}:{comment}", event.subject));
                        }
                        if let Some((from, to)) = proposed {
                            self.mail.push(format!("proposal:{}:{}/{}", event.subject, instant_text(from), instant_text(to)));
                        }
                        if answer == "decline" {
                            self.events.retain(|other| other.id != event.id);
                        } else if let Some(slot) = self.events.get_mut(at) {
                            slot.response = if answer == "accept" { "accepted".into() } else { "tentativelyAccepted".into() };
                            slot.version += 1;
                            slot.modified = self.clock;
                        }
                    }
                }
                Ok((202, None))
            }
            // A meeting sent on to more people. (The reference writes the two parameters and
            // what a recipient is made of with capitals here, and a recipient without them
            // where it describes one: either spelling is taken.)
            ("POST", [id, "forward"]) => {
                let body = asked.json();
                let Some(said) = body.as_object() else { return Err(refuse(400, "BadRequest", "Unable to read JSON request payload.")) };
                let any_case = |of: &Value, name: &str| -> Option<Value> {
                    of.as_object().and_then(|of| of.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))).map(|(_, value)| value.clone())
                };
                if let Some(other) = said.keys().find(|key| !key.eq_ignore_ascii_case("Comment") && !key.eq_ignore_ascii_case("ToRecipients")) {
                    return Err(bad(format!("The parameter '{other}' is not one of this action's.")));
                }
                let comment = match any_case(&body, "Comment") {
                    None => String::new(),
                    Some(Value::String(comment)) => comment,
                    Some(_) => return Err(bad("The parameter 'Comment' is a String.")),
                };
                let addressed = |recipient: &Value| {
                    let address = any_case(recipient, "EmailAddress").and_then(|address| any_case(&address, "Address"));
                    address.as_ref().and_then(Value::as_str).is_some_and(|address| address.contains('@') && !address.contains(char::is_whitespace))
                };
                let to = any_case(&body, "ToRecipients");
                let Some(to) = to.as_ref().and_then(Value::as_array).filter(|to| !to.is_empty() && to.iter().all(addressed)) else {
                    return Err(refuse(400, "ErrorInvalidRecipients", "At least one recipient isn't valid."));
                };
                // The event itself, or one day of a series as the series has it.
                let subject = match self.find(calendar, id) {
                    Ok(at) => self.events.get(at).map(|event| event.subject.clone()),
                    Err(why) => match self.occurrence_named(id) {
                        Some((series, _)) => self.events.get(series).map(|series| series.subject.clone()),
                        None => return Err(why),
                    },
                };
                self.mail.push(format!("forward:{}:{}:{comment}", subject.unwrap_or_default(), to.len()));
                Ok((202, None))
            }
            (_, [] | [_] | [_, "instances" | "cancel" | "accept" | "tentativelyAccept" | "decline" | "forward"]) => {
                Err(refuse(405, "MethodNotAllowed", "The method is not allowed on this resource."))
            }
            (_, [_, other, ..]) => Err(refuse(400, "BadRequest", format!("Resource not found for the segment '{other}'."))),
        }
    }

    /// Several requests in one (`POST /$batch`): each is answered by itself.
    fn batch(&mut self, asked: &Asked) -> Result<Answer, Refusal> {
        let body = asked.json();
        let Some(requests) = body.get("requests").and_then(Value::as_array) else {
            return Err(refuse(400, "BadRequest", "A batch has a list of requests."));
        };
        if requests.is_empty() || requests.len() > 20 {
            return Err(refuse(400, "BadRequest", "A batch holds between 1 and 20 requests."));
        }
        let mut ids: Vec<&str> = Vec::new();
        let mut responses: Vec<Value> = Vec::new();
        for request in requests {
            let (id, method, url) = (
                request.get("id").and_then(Value::as_str),
                request.get("method").and_then(Value::as_str),
                request.get("url").and_then(Value::as_str),
            );
            let (Some(id), Some(method), Some(url)) = (id, method, url) else {
                return Err(refuse(400, "BadRequest", "Every request of a batch has an id, a method and a url."));
            };
            if ids.contains(&id) {
                return Err(refuse(400, "BadRequest", "The ids of a batch's requests must be unique."));
            }
            ids.push(id);
            let mut headers: Vec<(String, String)> = vec![("authorization".into(), asked.header("authorization").to_string())];
            for (name, value) in request.get("headers").and_then(Value::as_object).into_iter().flatten() {
                headers.push((name.to_ascii_lowercase(), value.as_str().unwrap_or("").to_string()));
            }
            let body = request.get("body").map(Value::to_string).unwrap_or_default();
            let target = format!("/v1.0/{}", url.trim_start_matches('/'));
            let mut one = parse_target(method, &target, headers, body);
            let (status, body) = match self.answer(&one) {
                Ok((status, body)) => (status, body),
                Err(refusal) => (refusal.0, Some(error_body(&refusal))),
            };
            one.status = status;
            self.requests.push(one);
            // A request that is told to come back later says when among its own headers.
            let headers = if status == 429 || status == 503 {
                json!({"Content-Type": "application/json", "Retry-After": self.retry_after.unwrap_or(1).to_string()})
            } else {
                json!({"Content-Type": "application/json"})
            };
            responses.push(json!({"id": id, "status": status, "headers": headers, "body": body.unwrap_or(Value::Null)}));
        }
        Ok((200, Some(json!({"responses": responses}))))
    }
}

/// A refusal as Graph words it.
fn error_body(refusal: &Refusal) -> Value {
    json!({"error": {"code": refusal.1, "message": refusal.2, "innerError": {"date": "2026-10-12T08:00:00", "request-id": "00000000-0000-0000-0000-000000000000", "client-request-id": "00000000-0000-0000-0000-000000000000"}}})
}

/// A request as the stand-in looks at it: its path in pieces, its query in pairs.
fn parse_target(method: &str, target: &str, headers: Vec<(String, String)>, body: String) -> Asked {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Asked {
        method: method.to_string(),
        target: target.to_string(),
        path: path.split('/').filter(|part| !part.is_empty()).map(piece).collect(),
        query: crate::http::pairs(query),
        headers,
        body,
        bytes: Vec::new(),
        status: 0,
    }
}

/// Read one request from `stream`: the request line, the headers, and as much body as it says.
fn read_request(stream: &TcpStream) -> Option<Asked> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok().filter(|n| *n > 0)?;
    let mut words = line.split_whitespace();
    let (method, target) = (words.next()?.to_string(), words.next()?.to_string());
    let (mut headers, mut length) = (Vec::new(), 0usize);
    for _ in 0..200 {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim().to_string());
        if name == "content-length" {
            length = value.parse().ok().filter(|n| *n <= MAX_REQUEST)?;
        }
        headers.push((name, value));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    let mut asked = parse_target(&method, &target, headers, String::from_utf8_lossy(&body).into_owned());
    asked.bytes = body;
    Some(asked)
}

/// A stand-in for Graph's calendar on a loopback port, with the calendar it keeps.
pub struct StandIn {
    pub port: u16,
    state: Arc<Mutex<State>>,
}

impl StandIn {
    /// Start one: a main calendar called "Calendar" with nothing in it, for the owner
    /// `owner@example.org`, taking [`TOKEN`].
    pub fn start() -> std::io::Result<StandIn> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let mut state = State {
            token: TOKEN.into(),
            owner: "owner@example.org".into(),
            base: format!("http://127.0.0.1:{port}"),
            clock: 1_790_000_000,
            ..Default::default()
        };
        let main = state.add_calendar("Calendar", true, false);
        if let Some(calendar) = state.calendars.iter_mut().find(|calendar| calendar.id == main) {
            calendar.main = true;
        }
        // The folders of mail every mailbox has: the top, and the special ones under it. The
        // configuration of Outlook on the web is there, and has no list of Favorites yet.
        state.prefixes = ("a".into(), "b".into());
        let top = MailFolder {
            id: "AQMkAGV4YW1wbGUtb3JnLW1haWxib3gtZm9sZGVycwAuAAADk1v_Qx7RkS-nTtqB3L0d9EBAFh2c-Kp_0aTmWvN4uYx1RoAAAIBCAAAAA==".into(),
            name: "Top of Information Store".into(),
            parent: "AQMkAGV4YW1wbGUtb3JnLW1haWxib3gtZm9sZGVycwAuAAADk1v_Qx7RkS-nTtqB3L0d9EBAFh2c-Kp_0aTmWvN4uYx1RoAAAIBAQAAAA==".into(),
            well_known: "msgfolderroot".into(),
            ..Default::default()
        };
        state.folders.push(top);
        for (name, well_known) in SPECIAL_FOLDERS {
            let id = state.add_folder(name, None);
            if let Some(folder) = state.folders.iter_mut().find(|folder| folder.id == id) {
                folder.well_known = well_known.into();
            }
        }
        let state = Arc::new(Mutex::new(state));
        let shared = state.clone();
        std::thread::Builder::new().name("graph stand-in".into()).spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let Some(mut asked) = read_request(&stream) else { continue };
                let mut state = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let (status, body) = match state.answer(&asked) {
                    Ok(answer) => answer,
                    Err(refusal) => (refusal.0, Some(error_body(&refusal))),
                };
                asked.status = status;
                asked.bytes = Vec::new();
                // Only the latest requests are kept: a mailbox of 200,000 messages takes thousands.
                if state.requests.len() >= MAX_KEPT {
                    state.requests.drain(..MAX_KEPT / 2);
                }
                state.requests.push(asked);
                // Carried out, and the answer lost: the connection closes before a word of it.
                if std::mem::take(&mut state.lose_answer) {
                    state.location = None;
                    state.raw = None;
                    continue;
                }
                let wait = state.retry_after.unwrap_or(1);
                let location = state.location.take().map(|to| format!("Location: {to}\r\n")).unwrap_or_default();
                let raw = state.raw.take();
                drop(state);
                let reason = match status {
                    200 => "OK",
                    201 => "Created",
                    202 => "Accepted",
                    204 => "No Content",
                    400 => "Bad Request",
                    401 => "Unauthorized",
                    403 => "Forbidden",
                    404 => "Not Found",
                    405 => "Method Not Allowed",
                    409 => "Conflict",
                    410 => "Gone",
                    429 => "Too Many Requests",
                    _ => "Other",
                };
                let retry = if status == 429 || status == 503 { format!("Retry-After: {wait}\r\n") } else { String::new() };
                let answer = match (raw, body) {
                    (Some((kind, raw)), _) => {
                        format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{raw}", raw.len())
                    }
                    (None, Some(body)) => {
                        let body = body.to_string();
                        format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; odata.metadata=minimal\r\n{retry}{location}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    (None, None) => format!("HTTP/1.1 {status} {reason}\r\n{retry}{location}Content-Length: 0\r\nConnection: close\r\n\r\n"),
                };
                let _ = stream.write_all(answer.as_bytes());
            }
        })?;
        Ok(StandIn { port, state })
    }

    /// `http://127.0.0.1:{port}`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// What it keeps, to set up and to look at.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Have this thread's calls to Graph come here until the guard is dropped ([`route`]).
    pub fn route(&self) -> Routed {
        route(&self.url())
    }
}
