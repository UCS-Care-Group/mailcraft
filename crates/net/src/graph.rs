// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft Graph, for what a Microsoft 365 or Outlook.com account has that IMAP and SMTP don't
//! reach: [`send`] sends a message where SMTP is switched off for the account, [`events`] reads
//! the calendar and [`contacts`] the contacts.
//!
//! Each takes an access token for Graph, which is not the one the mail servers take:
//! [`oauth::refresh_for`](crate::oauth::refresh_for) with
//! [`Resource::Graph`](crate::oauth::Resource::Graph) gets one with the refresh token of the
//! sign-in. All of them block, for as long as the timeouts allow at most.
//!
//! Only `https://graph.microsoft.com` is ever spoken to: the address of a next page that a list
//! names is followed there and nowhere else, and redirects are not followed at all. What comes
//! back is hostile JSON: every answer, every list and every text in it is capped, what can't be
//! read is passed over, and nothing in it is run or fetched. The token and the mail never appear
//! in an error or a log line: an error says what was being done, the status, and at most the
//! short code Graph names its refusal by.
//!
//! What can go wrong, for the host to act on:
//! - `ProtoError::Auth` ([`NetError::is_refused_sign_in`]): Graph didn't take the token (401).
//!   Get a new one and ask again; when a new one is refused too, see `refresh_for`.
//! - [`NetError::Forbidden`]: the token is good and the account may not do this through Graph
//!   (403). Asking again won't help; mail can still try SMTP.
//! - [`NetError::Busy`]: not now (429, 503, 504). Ask again later, after as many seconds as it
//!   says when it says; nothing here waits.
//! - `ProtoError::Rejected`: Graph answered something else (a message it won't take, 400).
//! - Anything else: Graph couldn't be reached, or its answer made no sense.
//!
//! Events are written back too ([`push_events`]): one made here is created in the account's
//! main calendar (`POST /me/events`) or in another of its calendars
//! (`POST /me/calendars/{id}/events`), one changed here has what changed sent
//! (`PATCH /me/events/{id}`), one deleted here is deleted (`DELETE /me/events/{id}`). Each is
//! named by the id that came with it ([`mailcraft_model::Origin`]); a new one goes with a mark
//! (`transactionId`) that keeps a second try from making a second event. Contacts are only read.
//!
//! A series goes with its rule (`recurrence`) and its times by the clock in the owner's zone;
//! one occurrence of a series is found by its time (`GET /me/events/{id}/instances`) and then
//! changed or deleted as an event of its own. A meeting goes with who is invited, which makes
//! Exchange write to them: invitations for a new one, updates for a changed one, and a
//! cancellation when it is called off (`POST /me/events/{id}/cancel`, with a note). Somebody
//! else's invitation is answered with `/accept`, `/tentativelyAccept` or `/decline`. **Nothing
//! here decides to send mail**: an upload holds only what the mailbox was told to send
//! (`mailcraft-engine` refuses to note such a change without the owner's yes).
//!
//! An event also says how much it matters (`importance`), what the people invited are asked for
//! (`responseRequested`, `allowNewTimeProposals`) and whether it is held online: a new one is
//! made a Teams meeting with `isOnlineMeeting` and `onlineMeetingProvider`, and Graph's answer
//! carries the link to join by (`onlineMeeting.joinUrl`). An account that can't have one gets
//! the event without ([`mailcraft_model::remote::EventOutcome::online`] says so). An answer may
//! propose another time (`proposedNewTime` of `/tentativelyAccept` and `/decline`), and a
//! meeting is sent on with `POST /me/events/{id}/forward`: both are mail, and are in an upload
//! only on the owner's yes. Graph has no way of moving an event from one calendar to another:
//! the mailbox makes it anew and deletes the original, as two ops of its own.
//!
//! An event of a calendar other than the main one is written through that calendar
//! (`/me/calendars/{id}/events/{id}`): the copy of a calendar that somebody else shares with the
//! account is reached that way and no other. An event's id goes into an address as one piece of
//! it, with everything but letters, digits and `-._~` as `%XX` (an id may hold `=`, `/` and `+`).
//! The ids are Graph's ordinary ones, which change when an item is moved to another calendar:
//! a change that waits for such an item is answered "not found", and the mailbox says so.
//!
//! [`check_calendar`] makes, reads, changes, reads and deletes one appointment a year ahead, and
//! says what Graph answered at each step: the one way to see for oneself that what is written
//! here arrives. A host runs it when the owner asks, and never otherwise.
//!
//! Besides: every series of a stretch as itself ([`events_in`], twenty to a `$batch`), the
//! account's calendars and their groups ([`calendars`]), the mailbox's categories
//! ([`categories`], which takes `MailboxSettings.Read`), and when people are free and busy
//! ([`schedule`]).
//!
//! And the folders the owner keeps under Favorites in Outlook ([`favorites`], which takes
//! `MailboxConfigItem.Read` and `Mail.ReadBasic`): read from one configuration item of the
//! mailbox, in the beta version of Graph, and never written.

use std::borrow::Cow;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use mailcraft_charset::base64::STANDARD;
use mailcraft_mime::WriteOptions;
use mailcraft_model::remote::{
    BusyTime, CalendarsDownload, CategoriesDownload, ContactsDownload, EventChangeKind, EventFields, EventOp, EventOutcome, EventResult, EventTime,
    EventsDownload, EventsUpload, EventsUploaded, FreeBusy, FreeBusyAsk, MAX_BUSY, MAX_CONTACTS, MAX_EVENT_OPS, MAX_EVENTS, MAX_REMOTE_CALENDARS,
    MAX_REMOTE_CATEGORIES, MAX_SCHEDULE_STRETCH, MAX_SCHEDULES, PersonSchedule, RemoteCalendar, RemoteContact, RemoteEvent, RemoteEventKind,
};
use mailcraft_model::sync::SharedRefusal;
use mailcraft_model::{
    Address, Attendee, CalendarOwner, Category, Freq, Importance, Labeled, Message, OnlineMeeting, OnlineProvider, PostalAddress, Recurrence,
    Response, ResponseOptions, Rgb, Rights, ShowAs,
};
use mailcraft_proto::ProtoError;
use mailcraft_time::{Date, DateTime, Timestamp, Weekday};
use serde_json::{Map, Value, json};

use crate::NetError;
use crate::http::{self, Reply, Request, Url};

/// Where Graph is, and what is asked of it.
const GRAPH: &str = "https://graph.microsoft.com";

/// Where Graph is spoken to: its own address, always. (Only a test may have a stand-in on this
/// machine answer in its place, on its own thread: `standin::route`.)
pub(crate) fn graph_base() -> String {
    #[cfg(any(test, feature = "stand-in"))]
    if let Some(base) = standin::routed() {
        return base;
    }
    GRAPH.to_string()
}
const SEND_MAIL: &str = "/v1.0/me/sendMail";
#[cfg(test)]
const CALENDAR_VIEW: &str = "/v1.0/me/calendarView";
const CONTACTS: &str = "/v1.0/me/contacts";
const EVENTS: &str = "/v1.0/me/events";

/// What is asked for of each event, and of each contact: no more comes down than is kept.
/// (`lastModifiedDateTime` is not among them: a calendar view can't be asked for it by name.)
const EVENT_FIELDS: &str = "id,iCalUId,changeKey,subject,bodyPreview,body,start,end,isAllDay,isCancelled,location,organizer,attendees,\
                            responseStatus,showAs,sensitivity,type,seriesMasterId,isReminderOn,reminderMinutesBeforeStart,\
                            isOrganizer,transactionId";
/// The same without the two that were added last: asked for instead when Graph won't have the
/// list above (the calendar is still read then, as it was before events were written back).
const EVENT_FIELDS_PLAIN: &str = "id,iCalUId,changeKey,subject,bodyPreview,body,start,end,isAllDay,isCancelled,location,organizer,attendees,\
                                  responseStatus,showAs,sensitivity,type,seriesMasterId,isReminderOn,reminderMinutesBeforeStart";
/// And what is asked for first: with the categories, and when a series has each occurrence.
const EVENT_FIELDS_FULL: &str = "id,iCalUId,changeKey,subject,bodyPreview,body,start,end,isAllDay,isCancelled,location,organizer,attendees,\
                                 responseStatus,showAs,sensitivity,type,seriesMasterId,isReminderOn,reminderMinutesBeforeStart,\
                                 isOrganizer,transactionId,categories,originalStart,originalStartTimeZone,originalEndTimeZone";
/// What is asked for of a series itself: the same, and how it repeats.
const SERIES_FIELDS: &str = "id,iCalUId,changeKey,subject,bodyPreview,body,start,end,isAllDay,isCancelled,location,organizer,attendees,\
                             responseStatus,showAs,sensitivity,type,seriesMasterId,isReminderOn,reminderMinutesBeforeStart,\
                             isOrganizer,transactionId,categories,recurrence,originalStartTimeZone,originalEndTimeZone";
/// And first of all: with how much it matters, what the people invited are asked for, whether
/// it is held online and with what (and the link to join by), and whether files are kept with
/// it. A proposal of another time comes with the person who made it, among `attendees`.
const EVENT_FIELDS_OPTIONS: &str = "id,iCalUId,changeKey,subject,bodyPreview,body,start,end,isAllDay,isCancelled,location,organizer,attendees,\
                                    responseStatus,showAs,sensitivity,type,seriesMasterId,isReminderOn,\
                                    reminderMinutesBeforeStart,isOrganizer,transactionId,categories,originalStart,originalStartTimeZone,\
                                    originalEndTimeZone,importance,responseRequested,allowNewTimeProposals,isOnlineMeeting,\
                                    onlineMeetingProvider,onlineMeeting,onlineMeetingUrl,hasAttachments";
/// The same of a series itself.
const SERIES_FIELDS_OPTIONS: &str = "id,iCalUId,changeKey,subject,bodyPreview,body,start,end,isAllDay,isCancelled,location,organizer,attendees,\
                                     responseStatus,showAs,sensitivity,type,seriesMasterId,isReminderOn,\
                                     reminderMinutesBeforeStart,isOrganizer,transactionId,categories,recurrence,originalStartTimeZone,\
                                     originalEndTimeZone,importance,responseRequested,allowNewTimeProposals,isOnlineMeeting,\
                                     onlineMeetingProvider,onlineMeeting,onlineMeetingUrl,hasAttachments";
const BATCH: &str = "/v1.0/$batch";
const CALENDARS: &str = "/v1.0/me/calendars";
/// Most questions one batch carries (Graph's own limit), most series asked about in one go, and
/// the largest answer to a batch that is read.
const BATCH_SIZE: usize = 20;
const MAX_SERIES: usize = 200;
const MAX_BATCH: usize = 8 * 1024 * 1024;
/// How near to the time it is asked for by an occurrence has to be to be the one (seconds): a
/// timed one to the minute, a whole-day one within half a day (it is named by its day).
const NEAR: i64 = 90;
const NEAR_DAY: i64 = 12 * 3_600 - 1;
const CONTACT_FIELDS: &str = "id,changeKey,displayName,givenName,middleName,surname,nickName,companyName,jobTitle,department,emailAddresses,\
                              businessPhones,homePhones,mobilePhone,businessAddress,homeAddress,otherAddress,businessHomePage,birthday,\
                              personalNotes,lastModifiedDateTime";
/// How times and descriptions are asked for: in UTC, and as text rather than HTML.
const EVENT_PREFERENCES: &str = "outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"";
/// The names UTC goes by in an answer.
const UTC: [&str; 7] = ["UTC", "Etc/UTC", "GMT", "Etc/GMT", "Zulu", "Z", "tzone://Microsoft/Utc"];

/// How long Graph has to be reached and to answer one question, and to take a message.
const ASK_TIMEOUT: Duration = Duration::from_secs(30);
const SEND_TIMEOUT: Duration = Duration::from_secs(60);
/// How long Graph has to make, change or delete one event, and the largest answer to that which
/// is read (the event comes back whole).
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WRITTEN: usize = 4 * 1024 * 1024;
/// Longest name of a time zone that is sent.
const MAX_ZONE: usize = 64;
/// How long all the pages of one list may take together.
const LIST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Entries asked for per page, the largest page taken, and the most pages of one list.
const PAGE_SIZE: &str = "50";
const MAX_PAGE: usize = 4 * 1024 * 1024;
const MAX_PAGES: usize = 400;
/// Most events and contacts taken in one go; a list that has more is left incomplete.
const MAX_LISTED_EVENTS: usize = if MAX_EVENTS < 10_000 { MAX_EVENTS } else { 10_000 };
const MAX_LISTED_CONTACTS: usize = if MAX_CONTACTS < 10_000 { MAX_CONTACTS } else { 10_000 };
/// Most people read from one event, and most addresses or numbers of one kind from a contact.
const MAX_PEOPLE: usize = 500;
const MAX_ENTRIES: usize = 50;
/// The largest message that is sent (as it goes over the wire, before base64).
const MAX_MESSAGE: usize = 150 * 1024 * 1024;
/// Longest token sent, longest date read, longest address sent to, longest refusal code shown.
const MAX_TOKEN: usize = 16 * 1024;
const MAX_DATE: usize = 40;
const MAX_ADDRESS: usize = 254;
const MAX_CODE: usize = 64;
/// The longest run of characters a refusal code may have in common with what was sent.
const MAX_ECHO: usize = 8;
/// The longest Graph is believed when it says how long to stay away (seconds).
const MAX_WAIT: u64 = 24 * 3_600;

/// How [`send`] sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendOptions {
    /// The zone the `Date` header is written in, in minutes east of UTC; `None` for this
    /// machine's own zone at the message's date.
    pub offset_minutes: Option<i32>,
}

/// Send one message through Microsoft Graph, as the owner of `token` (an access token for
/// Graph). Graph also files a copy in the account's Sent Items.
///
/// The message goes as it would over SMTP (`mailcraft_mime::write`), in base64. Graph has no
/// envelope: it reads who the message is for from the message's own header fields. So the `Bcc`
/// field is written here, which it never is for SMTP; Exchange delivers to the people it names
/// and leaves the field out of what everybody else gets.
///
/// A message that can't be sent (nobody to send it to, an address that isn't one) is refused
/// before anything goes anywhere.
pub fn send(token: &str, message: &Message, options: &SendOptions) -> Result<(), NetError> {
    send_at(&graph_base(), token, message, options)
}

/// The events of the account's main calendar that fall between `from` and `to` (Unix seconds),
/// for the owner of `token` (an access token for Graph).
///
/// A series comes as its occurrences in that stretch, one event each, so no rule for repeating
/// comes with them. Times are UTC, for the mailbox to turn into its own wall-clock times.
/// [`EventsDownload::complete`] is false when not everything could be taken (the calendar has
/// more in the stretch than is read at once, or an event couldn't be read): what is there is
/// good, but nothing should be removed for being missing from it.
///
/// The download doesn't know whose it is: say so with [`EventsDownload::for_account`] before
/// the mailbox takes it in.
pub fn events(token: &str, from: Timestamp, to: Timestamp) -> Result<EventsDownload, NetError> {
    events_at(&graph_base(), token, from, to)
}

/// What [`events_in`] is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventsQuery {
    /// What the server calls the calendar; empty for the account's main one.
    pub calendar: String,
    /// The stretch (Unix seconds): from, and up to.
    pub from: Timestamp,
    pub to: Timestamp,
    /// Series the mailbox has that should happen in the stretch, by what the server calls them:
    /// those of them the stretch brings nothing of are asked about by name, so that one the
    /// server no longer has is known to be gone ([`EventsDownload::gone`]), and one that was cut
    /// short comes with its new end.
    pub series: Vec<String>,
    /// For a shared mailbox's calendar (#70): the mailbox's address. Everything is then asked on
    /// `/users/{address}/…` and nothing on `/me`. Empty for the account's own.
    pub shared: String,
}

/// [`events`] of any calendar of the account, with every series that happens in the stretch as
/// itself too: its rule, when that is one the mailbox can keep
/// ([`RemoteEvent::recurrence`](mailcraft_model::remote::RemoteEvent::recurrence)), beside its
/// occurrences in the stretch. The series are asked for together, twenty to a request.
///
/// A series that couldn't be asked about just now is left out, and the download is as complete
/// as it was: its occurrences are there, and the mailbox keeps the series as it has it.
pub fn events_in(token: &str, query: &EventsQuery) -> Result<EventsDownload, NetError> {
    events_in_at(&graph_base(), token, query)
}

/// The contacts in the account's contacts folder, for the owner of `token` (an access token for
/// Graph). [`ContactsDownload::complete`] and [`ContactsDownload::for_account`] are as for
/// [`events`].
pub fn contacts(token: &str) -> Result<ContactsDownload, NetError> {
    contacts_at(&graph_base(), token)
}

/// How events are written ([`push_events`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventOptions {
    /// The owner's time zone by a name Graph knows (`Pacific Standard Time`, `Europe/Berlin`).
    /// The days of a whole-day event are days there: without it they are days in UTC, which a
    /// program that shows them by the clock draws across two days for anyone not in UTC. Empty
    /// for UTC. (An event with times goes as instants, which need no zone.)
    pub zone: String,
    /// For a shared mailbox's calendars (#70): the mailbox's address. Every change is then
    /// written on `/users/{address}/…` and none on `/me`. Empty for the account's own.
    pub shared: String,
}

/// Tell Graph what was done here to the events of the account's main calendar, as the owner of
/// `token` (an access token for Graph): each op of the upload in turn, with what Graph made of
/// it ([`EventOutcome`]).
///
/// What a status means, for the mailbox to act on:
/// - made (201), changed (200), deleted (204), or any other 2xx: [`EventResult::Done`], with
///   the event's id and change key as they are now, when the answer says.
/// - 404 or 410 to a change or a deletion: [`EventResult::Gone`], the server no longer has it.
/// - 403: [`EventResult::Denied`], the account may not write to its calendar (the owner agreed
///   to reading only, or it can't be had).
/// - 429, 503, 504, any other 5xx, no connection, an answer that makes no sense:
///   [`EventResult::Failed`], to be tried again; after the first two (Graph is busy) and when
///   Graph can't be reached the rest of the upload is left untried.
/// - 409 to a new event: [`EventResult::Failed`] too (Graph has an event with that mark
///   already: the next download brings it, and it is known by the mark).
/// - any other refusal (400, 409 to a change, 412, 413…): [`EventResult::Refused`], for good.
/// - 401: nothing more is tried, and [`EventsUploaded::token_refused`] says so.
///
/// Nothing of the token or of an event appears in an outcome's error.
pub fn push_events(token: &str, upload: &EventsUpload, options: &EventOptions) -> EventsUploaded {
    push_events_at(&graph_base(), token, upload, options)
}

/// What Graph was being asked for when it said no, for the words of the error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Doing {
    Sending,
    Calendar,
    Contacts,
    Writing,
    Categories,
    Favorites,
    Folders,
    Mail,
    Tasks,
}

impl Doing {
    /// One word for a log line.
    fn word(self) -> &'static str {
        match self {
            Doing::Sending => "sending",
            Doing::Calendar | Doing::Writing => "calendar",
            Doing::Contacts => "contacts",
            Doing::Categories => "categories",
            Doing::Favorites => "favorites",
            Doing::Folders => "folders",
            Doing::Mail => "mail",
            Doing::Tasks => "tasks",
        }
    }
}

fn nonsense(text: &str) -> NetError {
    NetError::Proto(ProtoError::Protocol(text.into()))
}

/// How a run gets a new access token when Graph stops taking the one it has part-way (#306):
/// from the sign-in's refresh token, as the job got its first.
pub type Renew<'a> = &'a dyn Fn() -> Result<String, NetError>;

/// The access token of a run of mail that may be renewed, once, when Graph answers a request
/// 401 part-way (a token that ran out while the run went on, or while the computer slept): the
/// request is asked again with the new token, and a second 401 ends the run as before. Shared by
/// every [`Graph`] of the run; nothing of the token is logged.
pub(crate) struct Bearer<'a> {
    token: std::cell::RefCell<String>,
    renew: Option<Renew<'a>>,
    renewed: std::cell::Cell<bool>,
}

impl<'a> Bearer<'a> {
    pub(crate) fn new(token: &str, renew: Option<Renew<'a>>) -> Bearer<'a> {
        Bearer { token: std::cell::RefCell::new(token.to_string()), renew, renewed: std::cell::Cell::new(false) }
    }

    /// The token as it is now.
    fn current(&self) -> String {
        self.token.borrow().clone()
    }

    /// Renew the token, the first time only: whether there is a new one to ask again with.
    fn renew_once(&self) -> bool {
        let Some(renew) = self.renew else { return false };
        if self.renewed.replace(true) {
            return false;
        }
        match renew() {
            Ok(token) if usable_token(&token) => {
                log::info!("graph: Microsoft Graph stopped taking the sign-in part-way; it was renewed and the request is asked again");
                *self.token.borrow_mut() = token;
                true
            }
            Ok(_) => {
                log::warn!("graph: Microsoft Graph stopped taking the sign-in part-way, and the renewed one can't be shown to it");
                false
            }
            Err(e) => {
                log::warn!("graph: Microsoft Graph stopped taking the sign-in part-way, and it couldn't be renewed: {e}");
                false
            }
        }
    }
}

/// Whether `token` may be shown to Graph: not empty, not too long, and what a header can carry.
fn usable_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN && token.bytes().all(|b| b.is_ascii_graphic())
}

/// Graph as one caller has it: where it is, and the token that says who is asking.
struct Graph<'a> {
    base: Url,
    token: &'a str,
    /// The run's token when it may be renewed part-way (#306): then it, not `token`, is shown.
    bearer: Option<&'a Bearer<'a>>,
    /// Whether every request asks for immutable ids (`Prefer: IdType="ImmutableId"`): mail only, R3 spec S4.
    immutable: bool,
    /// The ground rules of a mailbox's mail ([`pace`]): its gate, who is waiting, and how a
    /// request is asked again. `None`: one try, as it always was.
    pace: Option<Pace>,
    /// Whose mailbox the mail is asked of: `/me`, the owner of the token; or `/users/{address}`,
    /// a shared mailbox the owner may open (#70, [`Graph::of_shared`]).
    mailbox: String,
    /// Requests asked so far, counted for the run's timings (#276): every try, retries too.
    requests: std::sync::atomic::AtomicU32,
}

/// What stands in an address of Graph's for a shared mailbox (#70): `/users/{address}`, the
/// address checked to be a plain one first ([`mailcraft_model::mailbox_address`]), so that it
/// names that mailbox and nothing else. Its `@` stays as it is (a path may have one), and
/// anything of the local part that isn't a letter, a digit or `-._~` is written as `%XX`.
pub(crate) fn shared_mailbox_path(address: &str) -> Result<String, NetError> {
    let refused = || NetError::Proto(ProtoError::Protocol("that isn't an address a shared mailbox can be opened at".into()));
    let address = mailcraft_model::mailbox_address(address).ok_or_else(refused)?;
    let (local, domain) = address.split_once('@').ok_or_else(refused)?;
    Ok(format!("/users/{}@{}", http::encode(local), http::encode(domain)))
}

/// What [`Graph::call`] goes by.
struct Pace {
    gate: std::sync::Arc<pace::Gate>,
    priority: pace::Priority,
    retry: pace::Retry,
}

/// Why a shared mailbox won't be opened, when an answer of Graph's says so by its own code (#70):
/// a 403 `ErrorAccessDenied`, a 404 `ErrorInvalidUser`, or `MailboxNotEnabledForRESTAPI` (and
/// its twin `MailboxNotSupportedForRESTAPI`). Any other code (a quota, a consent, a folder that
/// isn't there) is none of these.
pub(crate) fn shared_refusal(status: u64, body: &Value) -> Option<SharedRefusal> {
    let code = body.get("error").and_then(|error| error.get("code")).and_then(Value::as_str).unwrap_or("");
    match (status, code) {
        (_, "MailboxNotEnabledForRESTAPI" | "MailboxNotSupportedForRESTAPI") => Some(SharedRefusal::NotThroughGraph),
        (403, "ErrorAccessDenied") => Some(SharedRefusal::NoAccess),
        (404, "ErrorInvalidUser") => Some(SharedRefusal::NoMailbox),
        _ => None,
    }
}

/// What `Prefer` asks for of the ids in every answer to the Graph code of mail.
const IMMUTABLE_IDS: &str = "IdType=\"ImmutableId\"";

impl<'a> Graph<'a> {
    /// Graph at `base` (its own address, or a stand-in on this machine in the tests), for `token`.
    ///
    /// For the calendar, the contacts, sending and the Favorites: every request is tried once,
    /// and ids come as Graph's ordinary ones, as the mailbox files of 0.2.x keep them for events
    /// and contacts. (Immutable ids there would make every event and contact a mailbox has look
    /// gone and new; that change waits for a step that re-keys them.)
    fn at(base: &str, token: &'a str) -> Result<Graph<'a>, NetError> {
        // A token that isn't one is not sent anywhere; it is also what a header can't carry.
        if !usable_token(token) {
            return Err(NetError::Proto(ProtoError::Auth("there is no sign-in to show Microsoft Graph".into())));
        }
        Ok(Graph { base: Url::parse(base)?, token, bearer: None, immutable: false, pace: None, mailbox: "/me".into(), requests: Default::default() })
    }

    /// The same Graph, showing the run's `bearer` (which may be renewed once part-way, #306)
    /// instead of the token it was made with.
    fn renewing(self, bearer: Option<&'a Bearer<'a>>) -> Graph<'a> {
        Graph { bearer, ..self }
    }

    /// The token shown with every request now.
    fn token(&self) -> String {
        self.bearer.map_or_else(|| self.token.to_string(), Bearer::current)
    }

    /// Graph for a Microsoft account's mail (R3): every request asks for immutable ids, goes
    /// through the mailbox's `gate` and is asked again as [`pace`] has it.
    fn for_mail(
        base: &str,
        token: &'a str,
        gate: std::sync::Arc<pace::Gate>,
        priority: pace::Priority,
        retry: pace::Retry,
    ) -> Result<Graph<'a>, NetError> {
        let graph = Graph::at(base, token)?;
        Ok(Graph { immutable: true, pace: Some(Pace { gate, priority, retry }), ..graph })
    }

    /// [`Graph::ask`] by the ground rules of mail ([`pace`]), when this Graph has them: a place
    /// at the mailbox's gate first; a "busy" answer pauses the gate for as long as the server
    /// asked and, for a request of a `kind` that may be repeated, is asked again after it; so is
    /// one that couldn't reach the server. What comes back is the last answer, whatever its
    /// status, or why there was none.
    fn call(
        &self,
        kind: pace::Kind,
        url: &Url,
        method: &str,
        more: &[(&str, &str)],
        body: Option<&str>,
        patience: Duration,
        max_body: usize,
    ) -> Result<Reply, NetError> {
        let Some(pace) = &self.pace else { return self.ask_as(kind, url, method, more, body, patience, max_body) };
        let mut tries = 0u32;
        loop {
            tries = tries.saturating_add(1);
            let permit = pace.gate.enter(pace.priority, pace.retry.longest_wait)?;
            let asked = self.ask_as(kind, url, method, more, body, patience, max_body);
            drop(permit);
            let again = kind.may_repeat() && tries < pace.retry.tries;
            match asked {
                // The token stopped working part-way (#306): renewed once, and the request asked
                // again (Graph did nothing with it). A second 401 is the answer.
                Ok(reply) if reply.status == 401 && self.bearer.is_some_and(Bearer::renew_once) => {
                    tries = tries.saturating_sub(1);
                }
                Ok(reply) if matches!(reply.status, 429 | 503 | 504) => {
                    let wait = pace.retry.wait_before(tries.saturating_add(1), wait_of(reply.retry_after.as_deref(), mailcraft_time::now()));
                    // Everybody asking this mailbox waits, not just this request.
                    pace.gate.pause(wait);
                    log::debug!("graph: busy (status {}), every request of the mailbox waits {} s", reply.status, wait.as_secs());
                    if !again || wait > pace.retry.longest_wait {
                        return Ok(reply);
                    }
                }
                Err(e) if again && e.is_unreachable() => {
                    let wait = pace.retry.wait_before(tries.saturating_add(1), None);
                    log::debug!("graph: no answer; asking again in {} s", wait.as_secs());
                    std::thread::sleep(wait);
                }
                other => return other,
            }
        }
    }

    /// The same Graph, asking for the mail of the shared mailbox at `address` instead of the
    /// owner's own, when there is one (#70): `/users/{address}/…` for `/me/…`.
    fn of_shared(self, address: Option<&str>) -> Result<Graph<'a>, NetError> {
        match address {
            None => Ok(self),
            Some(address) => Ok(Graph { mailbox: shared_mailbox_path(address)?, ..self }),
        }
    }

    /// Whether the mail asked for is a shared mailbox's.
    fn shared(&self) -> bool {
        self.mailbox != "/me"
    }

    /// `rest` of the mailbox the mail is asked of, as a path of Graph v1.0: `/v1.0/me{rest}`, or
    /// `/v1.0/users/{address}{rest}` for a shared mailbox.
    fn mail_path(&self, rest: &str) -> String {
        format!("/v1.0{}{rest}", self.mailbox)
    }

    /// The same, as a request of a `$batch` names it (without the version).
    fn batch_path(&self, rest: &str) -> String {
        format!("{}{rest}", self.mailbox)
    }

    /// `path` (with its query) on Graph.
    fn url(&self, path: &str) -> Url {
        Url { path: path.to_string(), ..self.base.clone() }
    }

    /// One request, with the token. The answer comes back whatever its status.
    fn ask(
        &self,
        url: &Url,
        method: &str,
        more: &[(&str, &str)],
        body: Option<&str>,
        patience: Duration,
        max_body: usize,
    ) -> Result<Reply, NetError> {
        // Outside the ground rules of mail, only a GET is known to be one that may be asked again.
        self.with_headers(more, |headers| {
            self.send(url, &Request { method, headers, body: body.map(str::as_bytes), patience, max_body }, method == "GET")
        })
    }

    /// [`Graph::ask`] for a request of a `kind` the caller knows.
    #[allow(clippy::too_many_arguments)]
    fn ask_as(
        &self,
        kind: pace::Kind,
        url: &Url,
        method: &str,
        more: &[(&str, &str)],
        body: Option<&str>,
        patience: Duration,
        max_body: usize,
    ) -> Result<Reply, NetError> {
        self.with_headers(more, |headers| {
            self.send(url, &Request { method, headers, body: body.map(str::as_bytes), patience, max_body }, kind.may_repeat())
        })
    }

    /// One request on a connection kept open to Graph, or a new one ([`kept`](crate::kept)):
    /// only one that is `repeatable` goes on a connection that waited.
    fn send(&self, url: &Url, request: &Request<'_>, repeatable: bool) -> Result<Reply, NetError> {
        self.count_request();
        crate::kept::request(url, request, repeatable).map_err(|e| e.without(&self.token()))
    }

    /// The headers of every request, the token among them, with `more`, handed to `then`.
    fn with_headers<T>(&self, more: &[(&str, &str)], then: impl FnOnce(&[(&str, &str)]) -> T) -> T {
        let bearer = format!("Bearer {}", self.token());
        let mut headers = vec![("Authorization", bearer.as_str()), ("Accept", "application/json")];
        // Immutable ids, beside whatever else is preferred: one `Prefer` header, its values
        // parted by commas (RFC 7240).
        let preferred = more.iter().filter(|(name, _)| name.eq_ignore_ascii_case("Prefer")).map(|(_, value)| *value);
        let prefer = if self.immutable { std::iter::once(IMMUTABLE_IDS).chain(preferred).collect::<Vec<_>>().join(", ") } else { String::new() };
        if self.immutable {
            headers.push(("Prefer", prefer.as_str()));
            headers.extend(more.iter().filter(|(name, _)| !name.eq_ignore_ascii_case("Prefer")));
        } else {
            headers.extend_from_slice(more);
        }
        then(&headers)
    }

    /// Note one more request asked (the run's timings count them).
    fn count_request(&self) {
        let _ = self.requests.try_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |n| Some(n.saturating_add(1)));
    }

    /// How many requests this Graph has asked so far.
    fn requests_asked(&self) -> u32 {
        self.requests.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `PUT` a piece of a large attachment to the address an upload session gave (R3.4.1): through
    /// the mailbox's gate, and with no `Authorization`, as the address is its own permission (and
    /// the token goes to Graph alone). A piece is asked again only when the server said it was
    /// busy: one whose answer was lost may have been taken, and the next run starts the upload
    /// afresh, looking first whether the attachment is there.
    fn upload(&self, url: &Url, range: &str, piece: &[u8]) -> Result<Reply, NetError> {
        let headers = [("Content-Type", "application/octet-stream"), ("Content-Range", range)];
        let ask = || {
            self.count_request();
            http::request(url, &Request { method: "PUT", headers: &headers, body: Some(piece), patience: SEND_TIMEOUT, max_body: MAX_PAGE })
        };
        let Some(pace) = &self.pace else { return ask() };
        let mut tries = 0u32;
        loop {
            tries = tries.saturating_add(1);
            let permit = pace.gate.enter(pace.priority, pace.retry.longest_wait)?;
            let asked = ask();
            drop(permit);
            match asked {
                Ok(reply) if matches!(reply.status, 429 | 503) => {
                    let wait = pace.retry.wait_before(tries.saturating_add(1), wait_of(reply.retry_after.as_deref(), mailcraft_time::now()));
                    pace.gate.pause(wait);
                    if tries >= pace.retry.tries || wait > pace.retry.longest_wait {
                        return Ok(reply);
                    }
                }
                other => return other,
            }
        }
    }

    /// The error for an answer that isn't the one hoped for. `sent` is what went to Graph besides
    /// the token: none of it may come back in the error.
    fn refusal(&self, doing: Doing, reply: &Reply, sent: &str) -> NetError {
        // The mail's refusals are logged by status and Graph's code alone, never its message.
        if matches!(doing, Doing::Folders | Doing::Mail) {
            let code = logged_code(&reply.body, &[&self.token(), sent]);
            log::info!("graph {}: Microsoft Graph refused a request (status {}{code})", doing.word(), reply.status);
        }
        // A shared mailbox that won't let the owner in says so by Graph's code (#70).
        if self.shared() && matches!(doing, Doing::Folders | Doing::Mail) {
            let body = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            if let Some(why) = shared_refusal(u64::from(reply.status), &body) {
                return NetError::SharedRefused(why);
            }
        }
        refusal(doing, reply, &[&self.token(), sent], mailcraft_time::now())
    }

    /// Read a list page by page, handing every entry to `take`, `most` of them at most. Whether
    /// the list was read to its end.
    fn list(&self, doing: Doing, first: Url, more: &[(&str, &str)], most: usize, take: &mut dyn FnMut(&Value)) -> Result<bool, NetError> {
        let started = Instant::now();
        let mut taken = 0usize;
        let mut next = Some(first);
        for _ in 0..MAX_PAGES {
            let Some(url) = next.take() else { return Ok(true) };
            if started.elapsed() >= LIST_TIMEOUT {
                return Err(NetError::Proto(ProtoError::Io("Microsoft Graph took too long to answer".into())));
            }
            let reply = self.call(pace::Kind::Read, &url, "GET", more, None, ASK_TIMEOUT, MAX_PAGE)?;
            if reply.status != 200 {
                return Err(self.refusal(doing, &reply, ""));
            }
            let page = page_of(&reply.body)?;
            for entry in &page.entries {
                if taken >= most {
                    return Ok(false);
                }
                taken += 1;
                take(entry);
            }
            next = match &page.next {
                Some(link) => Some(self.next_page(link, &url)?),
                None => None,
            };
        }
        Ok(next.is_none())
    }

    /// The address of the page after `current`, when it is one that may be asked: on Graph
    /// itself, which is where the token belongs, and not the page it came from.
    fn next_page(&self, link: &Value, current: &Url) -> Result<Url, NetError> {
        let astray = || nonsense("Microsoft Graph named a next page that can't be followed");
        let url = Url::parse_within(link.as_str().ok_or_else(astray)?, http::MAX_LONG_URL).map_err(|_| astray())?;
        if self.base.same_server(&url) && url != *current { Ok(url) } else { Err(astray()) }
    }
}

/// One page of a list.
struct Page {
    entries: Vec<Value>,
    /// What the page says of where the list goes on (`@odata.nextLink`); `None` at its end.
    next: Option<Value>,
}

/// The entries of a page, and where it says the next one is: `{"value": […], "@odata.nextLink": "…"}`.
fn page_of(body: &[u8]) -> Result<Page, NetError> {
    page_from(serde_json::from_slice::<Value>(body).unwrap_or(Value::Null))
}

/// [`page_of`] of what was read already: the body of an answer inside a `$batch`.
fn page_from(json: Value) -> Result<Page, NetError> {
    let Value::Object(mut page) = json else { return Err(nonsense("Microsoft Graph's answer isn't the JSON it should be")) };
    let Some(Value::Array(entries)) = page.remove("value") else { return Err(nonsense("Microsoft Graph's answer has no list in it")) };
    Ok(Page { entries, next: page.remove("@odata.nextLink").filter(|link| !link.is_null()) })
}

/// The short name Graph gives a refusal (`{"error": {"code": "ErrorAccessDenied", …}}`), in
/// brackets for the end of a sentence; nothing when it has none, or when what stands in its
/// place isn't a name, or repeats what was `sent`: a careless server could write the token or
/// the mail back there. The message that comes with the name is never shown.
fn code_of(body: &[u8], sent: &[&str]) -> String {
    code_name(body, sent).map(|code| format!(" ({code})")).unwrap_or_default()
}

/// The same name as a log line carries it: `, code=ErrorInvalidRequest` (the form Copy
/// Diagnostics keeps whole), or nothing.
fn logged_code(body: &[u8], sent: &[&str]) -> String {
    code_name(body, sent).map(|code| format!(", code={code}")).unwrap_or_default()
}

/// The name of [`code_of`], when there is one that may be shown.
fn code_name(body: &[u8], sent: &[&str]) -> Option<String> {
    let json = serde_json::from_slice::<Value>(body).ok();
    let code = json.as_ref().and_then(|json| json.get("error")).and_then(|error| error.get("code")).and_then(Value::as_str).unwrap_or("");
    let named = !code.is_empty() && code.len() <= MAX_CODE && code.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.'));
    (named && !sent.iter().any(|sent| repeats(code, sent))).then(|| code.to_string())
}

/// Whether `code` (ASCII) repeats something of `sent`: it is a piece of it, or it has a run of
/// [`MAX_ECHO`] characters and more in common with it. No more of a token or of a message than
/// fits below that could come back in a name.
fn repeats(code: &str, sent: &str) -> bool {
    if code.len() <= MAX_ECHO {
        return sent.contains(code);
    }
    code.as_bytes().windows(MAX_ECHO).any(|run| std::str::from_utf8(run).is_ok_and(|run| sent.contains(run)))
}

/// How many seconds a `Retry-After` header asks for: a number of them, or the time to come back
/// at (RFC 9110 §10.2.3). No longer than a day.
fn wait_of(retry_after: Option<&str>, now: Timestamp) -> Option<u32> {
    let text = retry_after?.trim();
    let seconds = match text.parse::<u64>() {
        Ok(seconds) => seconds,
        Err(_) => u64::try_from(mailcraft_time::parse_rfc5322(text)?.0.saturating_sub(now)).unwrap_or(0),
    };
    u32::try_from(seconds.min(MAX_WAIT)).ok()
}

/// The error for an answer that isn't the one hoped for.
fn refusal(doing: Doing, reply: &Reply, sent: &[&str], now: Timestamp) -> NetError {
    let code = || code_of(&reply.body, sent);
    match reply.status {
        401 => NetError::Proto(ProtoError::Auth(format!("Microsoft Graph didn't take the sign-in{}", code()))),
        403 => {
            let what = match doing {
                Doing::Sending => "this account may not send through Microsoft Graph",
                Doing::Calendar => "this account may not read its calendar through Microsoft Graph",
                Doing::Contacts => "this account may not read its contacts through Microsoft Graph",
                Doing::Writing => "this account may not change its calendar through Microsoft Graph",
                Doing::Categories => "this account may not read its list of categories through Microsoft Graph",
                Doing::Favorites => "this account may not read its Favorites through Microsoft Graph",
                Doing::Folders => "this account may not read or change its folders through Microsoft Graph",
                Doing::Mail => "this account may not read its mail through Microsoft Graph",
                Doing::Tasks => "this account may not read or change its to-dos through Microsoft Graph",
            };
            NetError::Forbidden(format!("{what}{}", code()))
        }
        429 | 503 | 504 => {
            let retry_after = wait_of(reply.retry_after.as_deref(), now);
            let text = match retry_after {
                Some(1) => "Microsoft Graph is busy; try again in a second".to_string(),
                Some(seconds) => format!("Microsoft Graph is busy; try again in {seconds} seconds"),
                None => "Microsoft Graph is busy; try again later".to_string(),
            };
            NetError::Busy { text, retry_after }
        }
        413 if doing == Doing::Sending => NetError::Proto(ProtoError::Rejected("the message is too large to send through Microsoft Graph".into())),
        status => NetError::Proto(ProtoError::Rejected(format!("Microsoft Graph answered with status {status}{}", code()))),
    }
}

/// `local@domain` in visible ASCII: an address Graph can be handed in a header field.
fn is_address(email: &str) -> bool {
    let plain = email.len() <= MAX_ADDRESS && email.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b',' | b';' | b'"'));
    plain && email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty() && !domain.contains('@'))
}

/// [`send`] through Graph at `base`.
fn send_at(base: &str, token: &str, message: &Message, options: &SendOptions) -> Result<(), NetError> {
    let body = mime_body_with(message, options)?;
    let graph = Graph::at(base, token)?;
    let reply = graph.ask(&graph.url(SEND_MAIL), "POST", &[("Content-Type", "text/plain")], Some(&body), SEND_TIMEOUT, http::MAX_BODY)?;
    if reply.status == 202 { Ok(()) } else { Err(graph.refusal(Doing::Sending, &reply, &body)) }
}

/// What `sendMail` is handed for `message`: its MIME, with its Bcc, in base64, after the checks
/// every message goes through (someone to send it to, every address one).
fn mime_body(message: &Message) -> Result<String, NetError> {
    mime_body_with(message, &SendOptions::default())
}

fn mime_body_with(message: &Message, options: &SendOptions) -> Result<String, NetError> {
    // Leaving someone out silently would be worse than not sending.
    let mut anyone = false;
    for address in message.recipients() {
        let email = address.email.trim();
        if email.is_empty() {
            continue;
        }
        if !is_address(email) {
            let shown: String = email.chars().filter(|c| !c.is_control()).take(80).collect();
            return Err(NetError::Config(format!("\"{shown}\" isn't an address mail can be sent to")));
        }
        anyone = true;
    }
    if !anyone {
        return Err(NetError::Config("the message has no recipients".into()));
    }
    // A message that was never dated is dated now, as it is for SMTP.
    let message = if message.date == 0 { Cow::Owned(Message { date: mailcraft_time::now(), ..message.clone() }) } else { Cow::Borrowed(message) };
    let offset_minutes = options.offset_minutes.unwrap_or_else(|| mailcraft_time::local_offset_minutes(message.date));
    // With the Bcc field: it is the only way Graph learns of those it names.
    let mime = mailcraft_mime::write(&message, &WriteOptions { include_bcc: true, offset_minutes });
    if mime.len() > MAX_MESSAGE {
        return Err(NetError::Config("the message is too large to send through Microsoft Graph".into()));
    }
    Ok(STANDARD.encode(&mime))
}

/// [`events`] from Graph at `base`.
fn events_at(base: &str, token: &str, from: Timestamp, to: Timestamp) -> Result<EventsDownload, NetError> {
    events_in_at(base, token, &EventsQuery { from, to, ..Default::default() })
}

/// [`events_in`] from Graph at `base`.
fn events_in_at(base: &str, token: &str, query: &EventsQuery) -> Result<EventsDownload, NetError> {
    let (from, to) = (query.from, query.to);
    if to <= from {
        return Err(NetError::Config("the calendar was asked for a stretch of time with no length".into()));
    }
    let calendar = mailcraft_model::remote::clean_key(&query.calendar);
    if calendar.is_empty() != query.calendar.trim().is_empty() {
        return Err(NetError::Config("the calendar that was asked for isn't one the server could have".into()));
    }
    // A calendar opened by address is asked for at its owner's address and never at `/me` (#70):
    // it is that person's main calendar, so it is asked as a shared mailbox's main one is.
    if people::unsafe_id(&calendar) {
        return Err(NetError::Config("the calendar that was asked for isn't one that can be opened by address".into()));
    }
    let (shared, listed_calendar) = match people::address_of(&calendar) {
        Some(address) => (address, String::new()),
        None => (query.shared.clone(), calendar.clone()),
    };
    let graph = Graph::at(base, token)?.of_shared(Some(shared.as_str()).filter(|address| !address.trim().is_empty()))?;
    // Graph won't have the question as it is put (400): put as it was before the newest fields
    // were asked for, the calendar still comes.
    let mut listed = Err(nonsense("the calendar wasn't asked for"));
    let mut full = true;
    // (The fullest question comes first: what it adds to the next is `has_options`.)
    let mut options = true;
    for fields in [EVENT_FIELDS_OPTIONS, EVENT_FIELDS_FULL, EVENT_FIELDS, EVENT_FIELDS_PLAIN] {
        listed = events_with(&graph, &listed_calendar, from, to, fields);
        match &listed {
            Err(NetError::Proto(ProtoError::Rejected(why))) if why.starts_with(BAD_REQUEST) => {
                log::debug!("calendar: asked again with fewer fields");
                // The first refusal costs the newest fields, the next one the categories too.
                full &= options;
                options = false;
            }
            _ => break,
        }
    }
    // A somebody-else's main calendar that won't be listed to the owner (403: it is shared for
    // free and busy times only) is drawn from those instead, without what anything is (#70).
    // That holds for a shared mailbox's and for a calendar opened by address alike.
    if graph.shared() && listed_calendar.is_empty() && matches!(listed, Err(NetError::Forbidden(_))) {
        match free_busy_events(&graph, &shared, from, to) {
            Ok(mut download) => {
                download.calendar = calendar;
                return Ok(download);
            }
            Err(e) => log::debug!("calendar: free and busy times of somebody's calendar couldn't be had either: {e}"),
        }
    }
    let mut download = listed?;
    download.calendar = calendar;
    download.has_categories = full;
    download.has_options = options;
    // Every series that happens in the stretch, and those the mailbox expected to: as themselves.
    let mut wanted: Vec<String> = Vec::new();
    let named = download.events.iter().map(|event| &event.series).chain(&query.series);
    for series in named.map(|series| mailcraft_model::remote::clean_key(series)).filter(|series| !series.is_empty()) {
        if wanted.len() < MAX_SERIES && !wanted.contains(&series) {
            wanted.push(series);
        }
    }
    // (Asked of a series what could be asked of the events: no more, so that it comes at all.)
    let of_series = if options { SERIES_FIELDS_OPTIONS } else { SERIES_FIELDS };
    for (series, answer) in graph.series(&wanted, of_series) {
        match answer {
            Series::Found(event) => download.events.push(*event),
            Series::Gone => download.gone.push(series),
            Series::Unknown => {}
        }
    }
    Ok(download)
}

/// How the error for a 400 starts ([`refusal`]).
const BAD_REQUEST: &str = "Microsoft Graph answered with status 400";

/// What Graph said of a series that was asked for by name.
enum Series {
    Found(Box<RemoteEvent>),
    /// Graph no longer has it.
    Gone,
    /// It couldn't be asked, or the answer couldn't be read.
    Unknown,
}

impl Graph<'_> {
    /// Ask for the series called `ids` as themselves, [`BATCH_SIZE`] to a request. Every one
    /// gets an answer, in order.
    fn series(&self, ids: &[String], fields: &str) -> Vec<(String, Series)> {
        let mut out: Vec<(String, Series)> = Vec::new();
        let select = http::form(&[("$select", fields)]);
        for some in ids.chunks(BATCH_SIZE) {
            let requests: Vec<Value> = some
                .iter()
                .enumerate()
                .map(|(n, id)| {
                    json!({
                        "id": n.to_string(),
                        "method": "GET",
                        "url": self.batch_path(&format!("/events/{}?{select}", http::encode(id))),
                        "headers": {"Prefer": EVENT_PREFERENCES},
                    })
                })
                .collect();
            let body = json!({"requests": requests}).to_string();
            let mut answers: Vec<Series> = some.iter().map(|_| Series::Unknown).collect();
            let asked = self.ask(&self.url(BATCH), "POST", &[("Content-Type", "application/json")], Some(&body), ASK_TIMEOUT, MAX_BATCH);
            match asked {
                Ok(reply) if reply.status == 200 => {
                    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                    let said = json.get("responses").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
                    for one in said.iter().take(BATCH_SIZE * 2) {
                        // Each answer says which question it is to; one that doesn't is no answer.
                        let n = one.get("id").and_then(Value::as_str).and_then(|n| n.parse::<usize>().ok());
                        let Some((slot, id)) = n.and_then(|n| answers.get_mut(n).zip(some.get(n))) else { continue };
                        *slot = match one.get("status").and_then(Value::as_u64) {
                            Some(200) => one.get("body").and_then(|entry| series_of(entry, id)).map_or(Series::Unknown, Series::Found),
                            Some(404 | 410) => Series::Gone,
                            _ => Series::Unknown,
                        };
                    }
                }
                // Graph won't take questions together (or not put this way): one at a time.
                Ok(reply) if matches!(reply.status, 400 | 404 | 405 | 415 | 501) => {
                    log::debug!("calendar: the series are asked for one by one");
                    for (slot, id) in answers.iter_mut().zip(some) {
                        *slot = self.one_series(id, &select);
                    }
                }
                Ok(reply) => log::debug!("calendar: the series couldn't be asked for (status {})", reply.status),
                Err(_) => log::debug!("calendar: the series couldn't be asked for"),
            }
            out.extend(some.iter().cloned().zip(answers));
        }
        out
    }

    /// One series, asked for by itself.
    fn one_series(&self, id: &str, select: &str) -> Series {
        let url = self.url(&self.mail_path(&format!("/events/{}?{select}", http::encode(id))));
        match self.ask(&url, "GET", &[("Prefer", EVENT_PREFERENCES)], None, ASK_TIMEOUT, MAX_PAGE) {
            Ok(reply) if reply.status == 200 => {
                let entry = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                series_of(&entry, id).map_or(Series::Unknown, Series::Found)
            }
            Ok(reply) if matches!(reply.status, 404 | 410) => Series::Gone,
            _ => Series::Unknown,
        }
    }
}

/// The series in an answer, when it is the one that was asked for (`id`) and is a series.
fn series_of(entry: &Value, id: &str) -> Option<Box<RemoteEvent>> {
    let mut event = event_of(entry)?;
    // Whatever the answer calls it, it is the answer to this question; and whatever else it
    // says it is, it was asked for as a series.
    if event.kind != RemoteEventKind::SeriesMaster {
        return None;
    }
    event.id = id.to_string();
    event.series.clear();
    Some(Box::new(event))
}

/// [`events_at`], asking for `fields` of each event of `calendar` (empty: the main one).
fn events_with(graph: &Graph<'_>, calendar: &str, from: Timestamp, to: Timestamp, fields: &str) -> Result<EventsDownload, NetError> {
    let (start, end) = (mailcraft_time::format_iso_utc(from), mailcraft_time::format_iso_utc(to));
    let query = http::form(&[("startDateTime", &start), ("endDateTime", &end), ("$top", PAGE_SIZE), ("$select", fields)]);
    let mut download = EventsDownload { from, to, complete: true, ..Default::default() };
    let mut seen: HashSet<String> = HashSet::new();
    let mut unread = 0usize;
    let view = if calendar.is_empty() {
        graph.mail_path("/calendarView")
    } else {
        graph.mail_path(&format!("/calendars/{}/calendarView", http::encode(calendar)))
    };
    let first = graph.url(&format!("{view}?{query}"));
    let whole =
        graph.list(Doing::Calendar, first, &[("Prefer", EVENT_PREFERENCES)], MAX_LISTED_EVENTS, &mut |entry: &Value| match event_of(entry) {
            // A page that repeats an event doesn't make two of it.
            Some(event) if seen.insert(event.id.clone()) => download.events.push(event),
            Some(_) => {}
            None => unread += 1,
        })?;
    // Something that couldn't be read may be an event that is here already.
    download.complete = whole && unread == 0;
    log::debug!("calendar: {} events, {unread} that couldn't be read, complete: {}", download.events.len(), download.complete);
    Ok(download)
}

/// The name Graph has for how an event shows.
fn show_as_name(show_as: ShowAs) -> &'static str {
    match show_as {
        ShowAs::Free => "free",
        ShowAs::Tentative => "tentative",
        ShowAs::Busy => "busy",
        ShowAs::OutOfOffice => "oof",
        ShowAs::WorkingElsewhere => "workingElsewhere",
    }
}

/// `{"dateTime": "2026-10-12T13:00:00", "timeZone": "UTC"}`: an instant as Graph takes it. For a
/// whole-day event the instant is a midnight in UTC that names a day, and the day is one in
/// `zone` (UTC when that is empty).
fn date_time(instant: Timestamp, all_day: bool, zone: &str) -> Value {
    let named = if all_day { DateTime::at_midnight(DateTime::from_timestamp(instant, 0).date) } else { DateTime::from_timestamp(instant, 0) };
    let zone = if all_day && !zone.is_empty() { zone } else { "UTC" };
    json!({"dateTime": named.iso(), "timeZone": zone})
}

/// The name of a time zone, when `zone` can be one: letters, digits, spaces and `/_+-().`.
fn zone_name(zone: &str) -> &str {
    let zone = zone.trim();
    let fits = zone.len() <= MAX_ZONE && zone.chars().all(|c| c.is_ascii_alphanumeric() || " /_+-().".contains(c));
    if fits { zone } else { "" }
}

/// The event Graph is sent for `fields`: only what they say. `mark` goes with a new event.
/// `None` when it repeats in a way Graph has no way of saying.
///
/// `zone` is the zone the calendar is shown in, by a name Graph knows (empty: UTC). An event
/// with zones of its own ([`EventFields::zones`]) goes by their clocks and with their names when
/// `own` is set, which is how Graph keeps the zone an event was made in; without `own` (a second
/// try, after Graph wouldn't have a zone) it goes as instants in UTC, which is the same moment,
/// and a series that repeats by a zone's clock doesn't go at all: in UTC it would be another
/// series.
fn event_body(fields: &EventFields, mark: &str, zone: &str, own: bool) -> Option<String> {
    let mut event = Map::new();
    if let Some(title) = &fields.title {
        event.insert("subject".into(), json!(title));
    }
    if let Some(place) = &fields.location {
        event.insert("location".into(), json!({"displayName": place}));
    }
    if let Some(words) = &fields.description {
        event.insert("body".into(), json!({"contentType": "text", "content": words}));
    }
    if let Some(EventTime { start, end, all_day }) = fields.time {
        // A whole-day event is whole days: at least one, from midnight to midnight.
        let end = if all_day { end.max(start.saturating_add(mailcraft_time::DAY)) } else { end.max(start) };
        // The zones it has of its own, by the names Graph has for them.
        let zones = fields.zones.as_ref().filter(|_| !all_day);
        let named = zones.filter(|_| own).and_then(|zones| {
            let start = graph_zone(&zones.start)?;
            let end = if zones.end.is_empty() { start.clone() } else { graph_zone(&zones.end)? };
            Some((start, end))
        });
        match (named, fields.wall.filter(|_| !all_day)) {
            // Graph is to keep it in its zones: by their clocks. (A series comes with its
            // clock times; an event that happens once has them worked out.)
            (Some(((starts_in, start_name), (ends_in, end_name))), wall) => {
                let (from, to) = match wall {
                    Some(wall) => (wall.start, wall.end),
                    None => (starts_in.wall(start), ends_in.wall(end)),
                };
                event.insert("start".into(), json!({"dateTime": from.iso(), "timeZone": start_name}));
                event.insert("end".into(), json!({"dateTime": to.iso(), "timeZone": end_name}));
            }
            // A series by the clock of a zone Graph has no name for, or wouldn't have: it is
            // not sent by another clock. (An event that happens once goes as its instants.)
            (None, Some(_)) if zones.is_some() => return None,
            // A series: by the clock in the owner's zone, so that it stays at its hour all year.
            (None, Some(wall)) if !zone.is_empty() => {
                event.insert("start".into(), json!({"dateTime": wall.start.iso(), "timeZone": zone}));
                event.insert("end".into(), json!({"dateTime": wall.end.max(wall.start).iso(), "timeZone": zone}));
            }
            _ => {
                event.insert("start".into(), date_time(start, all_day, zone));
                event.insert("end".into(), date_time(end, all_day, zone));
            }
        }
        event.insert("isAllDay".into(), json!(all_day));
    }
    if let Some(repeat) = &fields.repeat {
        // The rule counts its days in the series' own zone, when it has one.
        let own_zone = fields.zones.as_ref().filter(|_| own).and_then(|zones| graph_zone(&zones.start)).map(|(_, name)| name);
        // Else it counts from the day the event starts on, as the event's start says it. Told
        // in UTC (no zone is known, or Graph didn't know it), that may be the day before or
        // after the one it is here, and every day of the rule moves with it.
        let in_utc = |time: &EventTime| own_zone.is_none() && !time.all_day && (zone.is_empty() || fields.wall.is_none());
        let shift = fields.time.filter(in_utc).map_or(0, |time| repeat.first.days_until(DateTime::from_timestamp(time.start, 0).date));
        let rule = match &repeat.rule {
            Some(rule) => recurrence_body(rule, repeat.first, own_zone.as_deref().unwrap_or(zone), shift)?,
            None => Value::Null,
        };
        event.insert("recurrence".into(), rule);
    }
    if let Some(people) = &fields.attendees {
        let invited: Vec<Value> = people
            .iter()
            .take(MAX_PEOPLE)
            .filter(|attendee| is_address(attendee.address.email.trim()))
            .map(|attendee| {
                json!({
                    "emailAddress": {"address": attendee.address.email.trim(), "name": attendee.address.name.trim()},
                    "type": if attendee.optional { "optional" } else { "required" },
                })
            })
            .collect();
        event.insert("attendees".into(), json!(invited));
    }
    if let Some(categories) = &fields.categories {
        event.insert("categories".into(), json!(categories));
    }
    if let Some(show_as) = fields.show_as {
        event.insert("showAs".into(), json!(show_as_name(show_as)));
    }
    if let Some(private) = fields.private {
        event.insert("sensitivity".into(), json!(if private { "private" } else { "normal" }));
    }
    if let Some(reminder) = fields.reminder {
        event.insert("isReminderOn".into(), json!(reminder.on));
        if reminder.on {
            event.insert("reminderMinutesBeforeStart".into(), json!(reminder.minutes));
        }
    }
    if let Some(importance) = fields.importance {
        event.insert("importance".into(), json!(importance.code()));
    }
    if let Some(asked) = fields.request_responses {
        event.insert("responseRequested".into(), json!(asked));
    }
    if let Some(allowed) = fields.allow_proposals {
        event.insert("allowNewTimeProposals".into(), json!(allowed));
    }
    // Held online: Graph makes the meeting and gives the link to join by in its answer.
    if let Some(provider) = fields.online {
        event.insert("isOnlineMeeting".into(), json!(true));
        event.insert("onlineMeetingProvider".into(), json!(provider.code()));
    }
    if !mark.is_empty() {
        event.insert("transactionId".into(), json!(mark));
    }
    Some(Value::Object(event).to_string())
}

/// What an answer to an invitation is sent with: the note and whether the organizer is told,
/// and another time when one is proposed with it (which Graph takes only with an answer that
/// is sent, and only for a tentative one or a decline).
fn answer_body(op: &EventOp) -> String {
    let mut answer = Map::new();
    answer.insert("comment".into(), json!(if op.send { op.comment.as_str() } else { "" }));
    answer.insert("sendResponse".into(), json!(op.send));
    if let Some(time) = op.proposed.filter(|_| op.send && matches!(op.response, Response::Tentative | Response::Declined)) {
        let end = time.end.max(time.start);
        answer.insert("proposedNewTime".into(), json!({"start": date_time(time.start, false, ""), "end": date_time(end, false, "")}));
    }
    Value::Object(answer).to_string()
}

/// What forwarding a meeting is sent with: who to (those of them with an address that is one),
/// and the note. `None` when nobody is left to send it to.
fn forward_body(op: &EventOp) -> Option<String> {
    let to: Vec<Value> = op
        .to
        .iter()
        .take(mailcraft_model::remote::MAX_FORWARD)
        .filter(|who| is_address(who.email.trim()))
        .map(|who| json!({"EmailAddress": {"Address": who.email.trim(), "Name": who.name.trim()}}))
        .collect();
    (!to.is_empty()).then(|| json!({"ToRecipients": to, "Comment": op.comment}).to_string())
}

/// What a Graph answer to making or changing an event says of its being held online: whether it
/// is, and the link to join it by (`onlineMeeting.joinUrl`, or the older `onlineMeetingUrl`).
fn online_of(entry: &Value) -> (Option<bool>, String) {
    let link = match entry.get("onlineMeeting").map_or("", |meeting| text(meeting, "joinUrl")) {
        "" => text(entry, "onlineMeetingUrl"),
        link => link,
    };
    let link = if link.len() <= mailcraft_model::MAX_JOIN_LINK { OnlineMeeting::clean_link(link).unwrap_or_default() } else { String::new() };
    (entry.get("isOnlineMeeting").and_then(Value::as_bool), link)
}

/// What became of one try at writing an event.
enum Wrote {
    /// Graph answered, or couldn't be asked: the outcome, and whether the rest of the upload is
    /// better left for later (Graph is busy, or can't be reached).
    Outcome(Box<EventOutcome>, bool),
    /// Graph didn't take the token: nothing more goes with it.
    TokenRefused,
}

/// [`push_events`] to Graph at `base`.
fn push_events_at(base: &str, token: &str, upload: &EventsUpload, options: &EventOptions) -> EventsUploaded {
    let mut uploaded = EventsUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    let Ok(graph) = Graph::at(base, token) else {
        uploaded.token_refused = true;
        return uploaded;
    };
    // A shared mailbox's calendars are written at its own address, and an address that is no
    // path of one writes nothing at all (and certainly nothing at `/me`).
    let shared = Some(options.shared.as_str()).filter(|address| !address.trim().is_empty());
    let graph = match graph.of_shared(shared) {
        Ok(graph) => graph,
        Err(e) => {
            for op in upload.ops.iter().take(MAX_EVENT_OPS) {
                uploaded.outcomes.push(EventOutcome { op: op.clone(), result: EventResult::Refused, error: e.to_string(), ..Default::default() });
            }
            return uploaded;
        }
    };
    // The zone the calendar is shown in, as the upload names it; else this computer's, as the
    // host knows it.
    let shown = graph_zone(&upload.zone).map(|(_, name)| name);
    let zone = shown.as_deref().unwrap_or_else(|| zone_name(&options.zone));
    for op in upload.ops.iter().take(MAX_EVENT_OPS) {
        let mut op = op.clone();
        let named = op.calendar.clone();
        op.tidy();
        // A calendar whose id can't be one is not the account's main calendar: nothing is sent
        // for it (the ids of calendars opened by address are checked the same way, #70).
        if !named.trim().is_empty() && op.calendar.is_empty() {
            let error = "that isn't a calendar the server could have".to_string();
            uploaded.outcomes.push(EventOutcome { op, result: EventResult::Refused, error, ..Default::default() });
            continue;
        }
        // A calendar opened by address is written at its owner's address: through a `Graph` of
        // its own, never the owner's (and never in an upload that is a shared mailbox's).
        let opened;
        let at = match people::address_of(&op.calendar) {
            None => &graph,
            Some(address) => match Graph::at(base, token).and_then(|g| g.of_shared(Some(address.as_str()))).ok().filter(|_| !graph.shared()) {
                Some(g) => {
                    opened = g;
                    &opened
                }
                None => {
                    let error = "that calendar can't be changed from here".to_string();
                    uploaded.outcomes.push(EventOutcome { op, result: EventResult::Refused, error, ..Default::default() });
                    continue;
                }
            },
        };
        match at.write_event(op, zone) {
            Wrote::Outcome(outcome, stop) => {
                uploaded.outcomes.push(*outcome);
                if stop {
                    break;
                }
            }
            Wrote::TokenRefused => {
                uploaded.token_refused = true;
                break;
            }
        }
    }
    log::debug!("calendar: {} of {} changes answered", uploaded.outcomes.len(), upload.ops.len());
    uploaded
}

impl Graph<'_> {
    /// Carry out one op, and say what Graph made of it.
    fn write_event(&self, op: EventOp, zone: &str) -> Wrote {
        let outcome = |op: EventOp, result: EventResult, error: String| Box::new(EventOutcome { op, result, error, ..Default::default() });
        // A change that says nothing: Graph is not asked, and nothing is called done that
        // wasn't. (The mailbox doesn't hand such a change over: it has nothing to send.)
        if op.kind == EventChangeKind::Update && op.fields.is_empty() {
            return Wrote::Outcome(outcome(op, EventResult::Refused, "the change says nothing that could be sent".into()), false);
        }
        // The events of the calendar it is in: the account's main one, or another by its name.
        // (A calendar opened by address is read through a `Graph` that is already at its owner's
        // address, whose main calendar it is: its events are `/users/{address}/events`.)
        if people::unsafe_id(&op.calendar) {
            return Wrote::Outcome(outcome(op, EventResult::Refused, "that isn't a calendar that can be opened by address".into()), false);
        }
        let events = if op.calendar.is_empty() || people::address_of(&op.calendar).is_some() {
            self.mail_path("/events")
        } else {
            self.mail_path(&format!("/calendars/{}/events", http::encode(&op.calendar)))
        };
        // One occurrence of a series, known by its time: Graph says what it calls it.
        let found = if op.of_occurrence() {
            match self.occurrence(&events, &op.series, op.occurrence) {
                Ok(Some(id)) => id,
                Ok(None) => return Wrote::Outcome(outcome(op, EventResult::Gone, "the series no longer has that occurrence".into()), false),
                Err(e) if e.is_refused_sign_in() => return Wrote::TokenRefused,
                Err(e) => {
                    let stop = !matches!(e, NetError::Proto(ProtoError::Rejected(_)) | NetError::Forbidden(_));
                    let result = match e {
                        NetError::Forbidden(_) => EventResult::Denied,
                        NetError::Proto(ProtoError::Rejected(_)) => EventResult::Refused,
                        _ => EventResult::Failed,
                    };
                    let mut said = outcome(op, result, e.to_string());
                    if let NetError::Busy { retry_after, .. } = e {
                        said.retry_after = retry_after;
                    }
                    return Wrote::Outcome(said, stop);
                }
            }
        } else {
            op.remote.clone()
        };
        let named = !found.is_empty();
        let event = format!("{events}/{}", http::encode(&found));
        let answer = match op.response {
            Response::Accepted => "accept",
            Response::Tentative => "tentativelyAccept",
            Response::Declined => "decline",
            Response::None => "",
        };
        let (method, path) = match op.kind {
            EventChangeKind::Create if !op.token.is_empty() => ("POST", events),
            EventChangeKind::Update if named => ("PATCH", event),
            // A meeting the owner called is called off, which tells the people invited.
            EventChangeKind::Delete if named && op.cancel => ("POST", format!("{event}/cancel")),
            EventChangeKind::Delete if named => ("DELETE", event),
            EventChangeKind::Respond if named && !answer.is_empty() => ("POST", format!("{event}/{answer}")),
            // A meeting sent on to more people, which only the owner's yes puts into an upload.
            EventChangeKind::Forward if named && op.send && !op.to.is_empty() => ("POST", format!("{event}/forward")),
            EventChangeKind::Forward => {
                return Wrote::Outcome(outcome(op, EventResult::Refused, "the forward doesn't say which meeting goes to whom".into()), false);
            }
            // Nothing says which event: not a question Graph can be asked.
            _ => return Wrote::Outcome(outcome(op, EventResult::Refused, "the change doesn't say which event it is about".into()), false),
        };
        let url = self.url(&path);
        let whole_day = op.fields.time.is_some_and(|time| time.all_day);
        let zoned = whole_day || op.fields.wall.is_some() || op.fields.repeat.is_some() || op.fields.zones.is_some();
        let mut zone = zone;
        // (Its own zones go with the first try; the second is without any.)
        let mut own = true;
        // The zone the times are said in, for whoever keeps a log: the event's own when it goes
        // by that, the owner's for the days of a whole-day event and the hours of a series, UTC
        // for everything else.
        let timed = matches!(op.kind, EventChangeKind::Create | EventChangeKind::Update) && op.fields.time.is_some();
        let by_the_clock = whole_day || op.fields.wall.is_some();
        let own_name = op.fields.zones.as_ref().filter(|_| !whole_day).and_then(|zones| {
            let (_, start) = graph_zone(&zones.start)?;
            if !zones.end.is_empty() {
                graph_zone(&zones.end)?;
            }
            Some(start)
        });
        let said_in = move |zone: &str, own: bool| match (timed, own_name.as_ref().filter(|_| own), by_the_clock && !zone.is_empty()) {
            (false, ..) => String::new(),
            (true, Some(name), _) => name.clone(),
            (true, None, true) => zone.to_string(),
            (true, None, false) => "UTC".to_string(),
        };
        // The event is to be held online. Graph may not have that for this account (a personal
        // one, a mailbox without Teams): it is then asked for the event without, and the
        // outcome says so, with what Graph said to the first asking.
        let asks_online = matches!(op.kind, EventChangeKind::Create | EventChangeKind::Update) && op.fields.online.is_some();
        let (mut online, mut no_online) = (asks_online, String::new());
        // A change says whether another time may be proposed. That is said when a meeting is
        // made; Graph's reference doesn't list it among what an update may say. Should Graph
        // refuse the change, it is asked once more without that, and the outcome says so: one
        // thing Graph won't have changed doesn't undo the rest of what was changed with it.
        let asks_new_times = op.kind == EventChangeKind::Update && op.fields.allow_proposals.is_some();
        let mut new_times = asks_new_times;
        // A few times at most: again without what Graph may not let an update say, again
        // without the online meeting, should it not have one, and again in UTC, should it not
        // know the zone a whole day is in.
        for _ in 0..5 {
            let body = match op.kind {
                EventChangeKind::Delete if op.cancel => Some(json!({"comment": op.comment}).to_string()),
                EventChangeKind::Delete => None,
                EventChangeKind::Respond => Some(answer_body(&op)),
                EventChangeKind::Forward => forward_body(&op),
                EventChangeKind::Create | EventChangeKind::Update => {
                    let mut fields = op.fields.clone();
                    if !online {
                        fields.online = None;
                    }
                    if asks_new_times && !new_times {
                        fields.allow_proposals = None;
                    }
                    // Nothing but what Graph won't have was to change: there is nothing left to
                    // ask, and the outcome says what was left out.
                    if op.kind == EventChangeKind::Update && fields.is_empty() {
                        let (no_meeting, no_times) = (asks_online && !online, asks_new_times && !new_times);
                        let mut done = outcome(op, EventResult::Done, if no_meeting { no_online } else { String::new() });
                        done.remote = found;
                        (done.online, done.new_times) = (no_meeting.then_some(false), no_times.then_some(false));
                        return Wrote::Outcome(done, false);
                    }
                    match event_body(&fields, &op.token, zone, own) {
                        Some(body) => Some(body),
                        None => {
                            let why = "it repeats in a way Microsoft 365 has no way of keeping";
                            return Wrote::Outcome(outcome(op, EventResult::Refused, why.into()), false);
                        }
                    }
                }
            };
            if op.kind == EventChangeKind::Forward && body.is_none() {
                return Wrote::Outcome(outcome(op, EventResult::Refused, "nobody it is forwarded to has an address".into()), false);
            }
            let headers: &[(&str, &str)] = if body.is_some() { &[("Content-Type", "application/json"), ("Prefer", EVENT_PREFERENCES)] } else { &[] };
            let reply = match self.ask(&url, method, headers, body.as_deref(), WRITE_TIMEOUT, MAX_WRITTEN) {
                Ok(reply) => reply,
                // Whether it went is not known. It is asked again later: a change is the same
                // change twice, and a new event carries its mark.
                Err(e) => return Wrote::Outcome(outcome(op, EventResult::Failed, e.to_string()), true),
            };
            let sent = format!("{} {} {found} {} {}", body.as_deref().unwrap_or(""), op.remote, op.series, op.calendar);
            let why = || self.refusal(Doing::Writing, &reply, &sent);
            let (result, stop) = match (op.kind, reply.status) {
                (_, 401) => return Wrote::TokenRefused,
                // Done, however Graph words its yes: taking a yes for a no would put back here
                // what the server has taken on.
                (_, 200..=299) => {
                    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                    // What an occurrence is called from now on, should the answer not say.
                    let changed_one = op.kind == EventChangeKind::Update && op.of_occurrence();
                    let zone_sent = said_in(zone, own);
                    let mut done = outcome(op, EventResult::Done, String::new());
                    (done.status, done.zone) = (reply.status, zone_sent);
                    done.remote = text(&json, "id").to_string();
                    if done.remote.is_empty() && changed_one {
                        done.remote = found;
                    }
                    done.change_key = match text(&json, "changeKey") {
                        "" => text(&json, "@odata.etag").to_string(),
                        key => key.to_string(),
                    };
                    done.uid = text(&json, "iCalUId").to_string();
                    // When Graph says it changed it: what a later answer is told from an earlier by.
                    done.modified = instant(text(&json, "lastModifiedDateTime")).unwrap_or(0);
                    // Whether it is held online now, for one that was to be: not, when Graph was
                    // asked for it without; else as its answer says, with the link to join by.
                    if asks_online {
                        let (is_online, link) = online_of(&json);
                        (done.online, done.join_url) = if online { (is_online, link) } else { (Some(false), String::new()) };
                        done.error = if done.online == Some(false) { no_online } else { String::new() };
                    }
                    // And whether what was said of proposing another time was left out for it.
                    if asks_new_times && !new_times {
                        done.new_times = Some(false);
                    }
                    done.tidy();
                    return Wrote::Outcome(done, false);
                }
                (EventChangeKind::Update | EventChangeKind::Delete | EventChangeKind::Respond | EventChangeKind::Forward, 404 | 410) => {
                    (EventResult::Gone, false)
                }
                (_, 403) => (EventResult::Denied, false),
                (_, 429 | 503 | 504) => (EventResult::Failed, true),
                (EventChangeKind::Create, 409) | (_, 500..=599) => (EventResult::Failed, false),
                // A change that says whether another time may be proposed, which Graph may not
                // let a change say: the same change without that, once. (Nothing was changed by
                // the one that was refused, and nobody was written to.)
                (EventChangeKind::Update, 400) if new_times => {
                    new_times = false;
                    continue;
                }
                // An online meeting that Graph won't make: the same event without one.
                (EventChangeKind::Create | EventChangeKind::Update, 400) if online => {
                    online = false;
                    no_online = match why() {
                        NetError::Proto(ProtoError::Rejected(words)) => words,
                        other => other.to_string(),
                    };
                    continue;
                }
                // A day or an hour in a zone Graph doesn't know: the same in UTC (and, as it
                // may have been the zone that was wrong, with the online meeting once more).
                (EventChangeKind::Create | EventChangeKind::Update, 400) if zoned && (!zone.is_empty() || (own && op.fields.zones.is_some())) => {
                    (zone, own) = ("", false);
                    online = asks_online;
                    continue;
                }
                _ => (EventResult::Refused, false),
            };
            let error = why();
            // In Graph's own words where it has any, without "the server said" before them.
            let words = match &error {
                NetError::Proto(ProtoError::Rejected(words)) => words.clone(),
                other => other.to_string(),
            };
            let zone_sent = said_in(zone, own);
            let mut said = outcome(op, result, words);
            (said.status, said.zone) = (reply.status, zone_sent);
            if let NetError::Busy { retry_after, .. } = error {
                said.retry_after = retry_after;
            }
            return Wrote::Outcome(said, stop);
        }
        Wrote::Outcome(outcome(op, EventResult::Refused, "Microsoft Graph wouldn't take the event".into()), false)
    }

    /// What Graph calls the occurrence that the series `series` (among `events`, the events of
    /// its calendar) has at `when` (Unix seconds): asked for among the occurrences of the day
    /// before and after. `None` when the series is
    /// gone, or has nothing then (it was deleted there, or the series was changed).
    fn occurrence(&self, events: &str, series: &str, time: Timestamp) -> Result<Option<String>, NetError> {
        if series.is_empty() || time == 0 {
            return Ok(None);
        }
        let (from, to) = (time.saturating_sub(mailcraft_time::DAY), time.saturating_add(mailcraft_time::DAY));
        let (start, end) = (mailcraft_time::format_iso_utc(from), mailcraft_time::format_iso_utc(to));
        let query =
            http::form(&[("startDateTime", &start), ("endDateTime", &end), ("$top", PAGE_SIZE), ("$select", "id,start,originalStart,isAllDay")]);
        let url = self.url(&format!("{events}/{}/instances?{query}", http::encode(series)));
        let reply = self.ask(&url, "GET", &[("Prefer", EVENT_PREFERENCES)], None, ASK_TIMEOUT, MAX_PAGE)?;
        match reply.status {
            200 => {}
            404 | 410 => return Ok(None),
            _ => return Err(self.refusal(Doing::Writing, &reply, series)),
        }
        let mut best: Option<(i64, String)> = None;
        for entry in page_of(&reply.body)?.entries.iter().take(200) {
            let id = mailcraft_model::remote::clean_key(text(entry, "id"));
            let all_day = entry.get("isAllDay").and_then(Value::as_bool).unwrap_or(false);
            // A whole-day one is named by its day; a timed one by when the series has it.
            let at = if all_day { when(entry.get("start")) } else { instant(text(entry, "originalStart")).or_else(|| when(entry.get("start"))) };
            let Some(off) = at.map(|at| at.saturating_sub(time).saturating_abs()) else { continue };
            let near = off <= if all_day { NEAR_DAY } else { NEAR };
            if !id.is_empty() && near && best.as_ref().is_none_or(|(least, _)| off < *least) {
                best = Some((off, id));
            }
        }
        Ok(best.map(|(_, id)| id))
    }
}

/// [`contacts`] from Graph at `base`.
fn contacts_at(base: &str, token: &str) -> Result<ContactsDownload, NetError> {
    let graph = Graph::at(base, token)?;
    let query = http::form(&[("$top", PAGE_SIZE), ("$select", CONTACT_FIELDS)]);
    let mut download = ContactsDownload { complete: true, ..Default::default() };
    let mut seen: HashSet<String> = HashSet::new();
    let mut unread = 0usize;
    let first = graph.url(&format!("{CONTACTS}?{query}"));
    let whole = graph.list(Doing::Contacts, first, &[], MAX_LISTED_CONTACTS, &mut |entry: &Value| match contact_of(entry) {
        Some(contact) if seen.insert(contact.id.clone()) => download.contacts.push(contact),
        Some(_) => {}
        None => unread += 1,
    })?;
    download.complete = whole && unread == 0;
    log::debug!("contacts: {} of them, {unread} that couldn't be read, complete: {}", download.contacts.len(), download.complete);
    Ok(download)
}

/// The text of `name` in a JSON object; empty when there is none, or it isn't text.
fn text<'v>(of: &'v Value, name: &str) -> &'v str {
    of.get(name).and_then(Value::as_str).unwrap_or("")
}

/// The instant a text names (ISO 8601, read as UTC when it names no zone of its own).
fn instant(text: &str) -> Option<Timestamp> {
    if text.len() > MAX_DATE { None } else { mailcraft_time::iso_to_timestamp(text, 0) }
}

/// The instant in `{"dateTime": "2026-10-12T13:00:00.0000000", "timeZone": "UTC"}`. UTC is what
/// was asked for; a time in a zone that would have to be looked up is not read.
fn when(value: Option<&Value>) -> Option<Timestamp> {
    let value = value?;
    let zone = text(value, "timeZone").trim();
    if !zone.is_empty() && !UTC.iter().any(|utc| utc.eq_ignore_ascii_case(zone)) {
        // Not what was asked for, but a zone that is known says when it is all the same. One
        // that isn't is not read: nothing is guessed.
        let known = Some(zone).filter(|zone| zone.len() <= MAX_ZONE).and_then(mailcraft_time::Zone::named)?;
        let written = Some(text(value, "dateTime")).filter(|text| text.len() <= MAX_DATE).and_then(mailcraft_time::parse_iso)?;
        return match written {
            (wall, None) => Some(known.instant(wall)),
            (wall, Some(offset)) => Some(wall.to_timestamp(offset)),
        };
    }
    instant(text(value, "dateTime"))
}

/// The name Graph knows a zone by, with the zone: the name Windows has for it when it has one
/// (which Exchange is sure to know), else its name in the IANA database, which Graph takes for
/// most zones. `None` for a name that is no zone, and for an offset, which Graph has no name for.
fn graph_zone(name: &str) -> Option<(mailcraft_time::Zone, String)> {
    let zone = Some(name).filter(|name| name.len() <= MAX_ZONE).and_then(mailcraft_time::Zone::named)?;
    let known = match zone.windows_name() {
        Some(windows) => windows.to_string(),
        None if zone.id().contains('/') => zone.id(),
        None => return None,
    };
    Some((zone, known))
}

/// The person in `{"emailAddress": {"name": "…", "address": "…"}}`, or in the inner object itself.
fn person(value: &Value) -> Address {
    let inner = value.get("emailAddress").unwrap_or(value);
    Address { name: text(inner, "name").to_string(), email: text(inner, "address").to_string() }
}

/// What somebody answered an invitation.
fn response(status: Option<&Value>) -> Response {
    match status.map_or("", |status| text(status, "response")).to_ascii_lowercase().as_str() {
        "accepted" | "organizer" => Response::Accepted,
        "tentativelyaccepted" => Response::Tentative,
        "declined" => Response::Declined,
        _ => Response::None,
    }
}

/// One entry of a calendar view as the mailbox can take it; `None` for one that has no id, or no
/// times that can be read.
fn event_of(entry: &Value) -> Option<RemoteEvent> {
    if !entry.is_object() {
        return None;
    }
    let (start, end) = (when(entry.get("start"))?, when(entry.get("end"))?);
    // The description as text when Graph sent text, as asked; otherwise the short text it makes of it.
    let body = entry.get("body").filter(|body| text(body, "contentType").eq_ignore_ascii_case("text")).map(|body| text(body, "content"));
    let attendees = entry.get("attendees").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let flag = |name: &str| entry.get(name).and_then(Value::as_bool).unwrap_or(false);
    let reminder = entry.get("reminderMinutesBeforeStart").and_then(Value::as_u64).map(|minutes| u32::try_from(minutes).unwrap_or(u32::MAX));
    // The zones it was made in, as Graph names them; a series counts by the zone of its rule.
    let zone = |name: &str| Some(name.trim()).filter(|name| name.len() <= MAX_ZONE).unwrap_or("").to_string();
    let rule_zone = entry.get("recurrence").and_then(|rule| rule.get("range")).map_or("", |range| text(range, "recurrenceTimeZone"));
    let start_zone = if rule_zone.trim().is_empty() { text(entry, "originalStartTimeZone") } else { rule_zone };
    let mut event = RemoteEvent {
        start_zone: zone(start_zone),
        end_zone: zone(text(entry, "originalEndTimeZone")),
        id: text(entry, "id").to_string(),
        change_key: match text(entry, "changeKey") {
            "" => text(entry, "@odata.etag").to_string(),
            key => key.to_string(),
        },
        uid: text(entry, "iCalUId").to_string(),
        title: text(entry, "subject").to_string(),
        location: entry.get("location").map_or("", |place| text(place, "displayName")).to_string(),
        description: body.filter(|body| !body.trim().is_empty()).unwrap_or_else(|| text(entry, "bodyPreview")).to_string(),
        start,
        end,
        all_day: flag("isAllDay"),
        organizer: entry.get("organizer").map(person),
        attendees: attendees
            .iter()
            .take(MAX_PEOPLE)
            .map(|attendee| Attendee {
                address: person(attendee),
                optional: text(attendee, "type").eq_ignore_ascii_case("optional"),
                response: response(attendee.get("status")),
            })
            .collect(),
        response: response(entry.get("responseStatus")),
        show_as: ShowAs::parse(text(entry, "showAs")).unwrap_or_default(),
        private: matches!(text(entry, "sensitivity").to_ascii_lowercase().as_str(), "private" | "confidential"),
        cancelled: flag("isCancelled"),
        kind: match text(entry, "type").to_ascii_lowercase().as_str() {
            "occurrence" => RemoteEventKind::Occurrence,
            "exception" => RemoteEventKind::Exception,
            "seriesmaster" => RemoteEventKind::SeriesMaster,
            _ => RemoteEventKind::Single,
        },
        series: text(entry, "seriesMasterId").to_string(),
        reminder_minutes: reminder.filter(|_| flag("isReminderOn")),
        modified: instant(text(entry, "lastModifiedDateTime")).unwrap_or(0),
        is_organizer: entry.get("isOrganizer").and_then(Value::as_bool),
        token: text(entry, "transactionId").to_string(),
        recurrence: None,
        series_day: None,
        original: instant(text(entry, "originalStart")),
        categories: entry
            .get("categories")
            .and_then(Value::as_array)
            .map(|names| names.iter().take(MAX_ENTRIES).filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        importance: Importance::parse(text(entry, "importance")).unwrap_or_default(),
        // Both are so unless Graph says they are not.
        options: ResponseOptions {
            request_responses: entry.get("responseRequested").and_then(Value::as_bool) != Some(false),
            allow_proposals: entry.get("allowNewTimeProposals").and_then(Value::as_bool) != Some(false),
        },
        online: online_meeting(entry),
        proposals: attendees.iter().take(MAX_PEOPLE).filter_map(proposal_of).collect(),
        attachments: flag("hasAttachments"),
    };
    if event.kind == RemoteEventKind::SeriesMaster
        && let Some((rule, day)) = entry.get("recurrence").and_then(rule_of)
    {
        (event.recurrence, event.series_day) = (Some(rule), Some(day));
    }
    event.tidy();
    (!event.id.is_empty()).then_some(event)
}

/// The online meeting an event is, when Graph says it is one: what it is held with, and the
/// link to join it by when there is one that can be a link.
fn online_meeting(entry: &Value) -> Option<OnlineMeeting> {
    let (online, join_url) = online_of(entry);
    if online != Some(true) && join_url.is_empty() {
        return None;
    }
    let provider = OnlineProvider::parse(text(entry, "onlineMeetingProvider")).unwrap_or(OnlineProvider::Other);
    Some(OnlineMeeting { provider, join_url })
}

/// Another time that one of the people invited proposes (`proposedNewTime` of an attendee).
fn proposal_of(attendee: &Value) -> Option<mailcraft_model::remote::RemoteProposal> {
    let proposed = attendee.get("proposedNewTime").filter(|proposed| proposed.is_object())?;
    let (start, end) = (when(proposed.get("start"))?, when(proposed.get("end"))?);
    Some(mailcraft_model::remote::RemoteProposal { from: person(attendee), start, end: end.max(start) })
}

/// The names Graph has for the days of the week, and for which one of a month is meant.
const DAYS: [(&str, Weekday); 7] = [
    ("monday", Weekday::Mon),
    ("tuesday", Weekday::Tue),
    ("wednesday", Weekday::Wed),
    ("thursday", Weekday::Thu),
    ("friday", Weekday::Fri),
    ("saturday", Weekday::Sat),
    ("sunday", Weekday::Sun),
];
const INDEXES: [(&str, i8); 5] = [("first", 1), ("second", 2), ("third", 3), ("fourth", 4), ("last", -1)];

/// How a series repeats (`{"pattern": {…}, "range": {…}}`) as the mailbox says it, with the day
/// the rule counts from; `None` for a rule the mailbox has no way of saying (the first weekday of
/// a month, a day of the month other than the one it starts on), which leaves the series to come
/// as its occurrences.
fn rule_of(recurrence: &Value) -> Option<(Recurrence, Date)> {
    let (pattern, range) = (recurrence.get("pattern")?, recurrence.get("range")?);
    let first = Some(text(range, "startDate")).filter(|day| day.len() <= MAX_DATE).and_then(Date::parse_iso)?;
    let number = |of: &Value, name: &str| of.get(name).and_then(Value::as_u64);
    let interval = u32::try_from(number(pattern, "interval").unwrap_or(1)).ok().filter(|n| (1..=999).contains(n))?;
    let mut by_day: Vec<Weekday> = Vec::new();
    for day in pattern.get("daysOfWeek").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(8) {
        let name = day.as_str()?.to_ascii_lowercase();
        by_day.push(DAYS.iter().find(|(known, _)| *known == name)?.1);
    }
    by_day.sort();
    by_day.dedup();
    let index = || INDEXES.iter().find(|(name, _)| name.eq_ignore_ascii_case(text(pattern, "index"))).map(|(_, nth)| *nth);
    // The mailbox counts a month's or a year's day from the day the series starts on.
    let on_its_day = || number(pattern, "dayOfMonth").is_none_or(|day| day == 0 || day == u64::from(first.day));
    let in_its_month = || number(pattern, "month").is_none_or(|month| month == 0 || month == u64::from(first.month));
    let one_day = by_day.len() == 1;
    let mut rule = match text(pattern, "type").to_ascii_lowercase().as_str() {
        "daily" => Recurrence { freq: Freq::Daily, ..Default::default() },
        "weekly" => Recurrence { freq: Freq::Weekly, by_day, ..Default::default() },
        "absolutemonthly" if on_its_day() => Recurrence { freq: Freq::Monthly, ..Default::default() },
        "relativemonthly" if one_day => Recurrence { freq: Freq::Monthly, by_day, nth: Some(index()?), ..Default::default() },
        "absoluteyearly" if on_its_day() && in_its_month() => Recurrence { freq: Freq::Yearly, ..Default::default() },
        "relativeyearly" if one_day && in_its_month() => Recurrence { freq: Freq::Yearly, by_day, nth: Some(index()?), ..Default::default() },
        _ => return None,
    };
    rule.interval = interval;
    match text(range, "type").to_ascii_lowercase().as_str() {
        "enddate" => rule.until = Some(Some(text(range, "endDate")).filter(|day| day.len() <= MAX_DATE).and_then(Date::parse_iso)?),
        "numbered" => rule.count = Some(u32::try_from(number(range, "numberOfOccurrences")?).ok().filter(|n| (1..=9_999).contains(n))?),
        "noend" => {}
        _ => return None,
    }
    Some((rule, first))
}

/// How a series repeats, as Graph is told: the rule, counted from the day `first` in `zone`
/// (UTC when that is empty). `shift` is how many days later (1) or earlier (-1) the event starts
/// as it is told than on `first`: the rule's days are then those days. `None` for a rule Graph
/// has no way of saying.
fn recurrence_body(rule: &Recurrence, first: Date, zone: &str, shift: i64) -> Option<Value> {
    let moved;
    let (rule, first) = if shift == 1 || shift == -1 {
        let mut by_day: Vec<Weekday> = rule.by_day.iter().map(|day| Weekday::from_index((i64::from(day.index()) + shift).rem_euclid(7))).collect();
        by_day.sort();
        moved = Recurrence { by_day, until: rule.until.map(|until| until.add_days(shift)), ..rule.clone() };
        (&moved, first.add_days(shift))
    } else {
        (rule, first)
    };
    let interval = rule.interval.clamp(1, 999);
    let name = |day: Weekday| DAYS.iter().find(|(_, known)| *known == day).map_or("monday", |(name, _)| *name);
    let own_day = [first.weekday()];
    let days = |list: &[Weekday]| -> Vec<&'static str> { if list.is_empty() { &own_day[..] } else { list }.iter().map(|day| name(*day)).collect() };
    let index = |nth: i8| INDEXES.iter().find(|(_, known)| *known == nth).map(|(name, _)| *name);
    let one_day = rule.by_day.len() <= 1;
    let pattern = match (rule.freq, rule.nth.filter(|nth| *nth != 0)) {
        (Freq::Daily, _) if rule.by_day.is_empty() => json!({"type": "daily", "interval": interval}),
        // "Every weekday": Graph says that as a week with those days in it.
        (Freq::Daily, _) if interval == 1 => json!({"type": "weekly", "interval": 1, "daysOfWeek": days(&rule.by_day), "firstDayOfWeek": "monday"}),
        (Freq::Daily, _) => return None,
        (Freq::Weekly, _) => json!({"type": "weekly", "interval": interval, "daysOfWeek": days(&rule.by_day), "firstDayOfWeek": "monday"}),
        (Freq::Monthly, None) => json!({"type": "absoluteMonthly", "interval": interval, "dayOfMonth": first.day}),
        (Freq::Monthly, Some(nth)) if one_day => {
            json!({"type": "relativeMonthly", "interval": interval, "daysOfWeek": days(&rule.by_day), "index": index(nth)?})
        }
        (Freq::Yearly, None) => json!({"type": "absoluteYearly", "interval": interval, "dayOfMonth": first.day, "month": first.month}),
        (Freq::Yearly, Some(nth)) if one_day => {
            json!({"type": "relativeYearly", "interval": interval, "daysOfWeek": days(&rule.by_day), "index": index(nth)?, "month": first.month})
        }
        _ => return None,
    };
    let zone = if zone.is_empty() { "UTC" } else { zone };
    let range = match (rule.count, rule.until) {
        (Some(count), _) => {
            json!({"type": "numbered", "startDate": first.iso(), "numberOfOccurrences": count.clamp(1, 9_999), "recurrenceTimeZone": zone})
        }
        (None, Some(until)) => json!({"type": "endDate", "startDate": first.iso(), "endDate": until.max(first).iso(), "recurrenceTimeZone": zone}),
        (None, None) => json!({"type": "noEnd", "startDate": first.iso(), "recurrenceTimeZone": zone}),
    };
    Some(json!({"pattern": pattern, "range": range}))
}

/// The postal address in a contact's `businessAddress`, `homeAddress` or `otherAddress`.
fn postal(entry: &Value, name: &str, label: &str) -> PostalAddress {
    let empty = Value::Null;
    let place = entry.get(name).unwrap_or(&empty);
    PostalAddress {
        label: label.to_string(),
        street: text(place, "street").to_string(),
        city: text(place, "city").to_string(),
        region: text(place, "state").to_string(),
        postal_code: text(place, "postalCode").to_string(),
        country: text(place, "countryOrRegion").to_string(),
    }
}

/// One entry of the contacts as the mailbox can take it; `None` for one without an id.
fn contact_of(entry: &Value) -> Option<RemoteContact> {
    if !entry.is_object() {
        return None;
    }
    let list = |name: &str| entry.get(name).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let numbers = |name: &str, label: &'static str| {
        list(name).iter().take(MAX_ENTRIES).filter_map(Value::as_str).map(move |number| Labeled::new(label, number))
    };
    let mut contact = RemoteContact {
        id: text(entry, "id").to_string(),
        change_key: match text(entry, "changeKey") {
            "" => text(entry, "@odata.etag").to_string(),
            key => key.to_string(),
        },
        display_name: text(entry, "displayName").to_string(),
        first: text(entry, "givenName").to_string(),
        middle: text(entry, "middleName").to_string(),
        last: text(entry, "surname").to_string(),
        nickname: text(entry, "nickName").to_string(),
        company: text(entry, "companyName").to_string(),
        title: text(entry, "jobTitle").to_string(),
        department: text(entry, "department").to_string(),
        emails: list("emailAddresses").iter().take(MAX_ENTRIES).map(|address| Labeled::new("", person(address).email)).collect(),
        phones: std::iter::once(Labeled::new("mobile", text(entry, "mobilePhone")))
            .chain(numbers("businessPhones", "work"))
            .chain(numbers("homePhones", "home"))
            .collect(),
        addresses: vec![postal(entry, "businessAddress", "work"), postal(entry, "homeAddress", "home"), postal(entry, "otherAddress", "other")],
        websites: vec![text(entry, "businessHomePage").to_string()],
        // The day as it is written: a birthday is a day, whatever time of it the server adds.
        birthday: Some(text(entry, "birthday")).filter(|day| day.len() <= MAX_DATE).and_then(mailcraft_time::parse_iso).map(|(day, _)| day.date),
        notes: text(entry, "personalNotes").to_string(),
        modified: instant(text(entry, "lastModifiedDateTime")).unwrap_or(0),
    };
    // Entries with nothing in them (no mobile number, no home address) go here.
    contact.tidy();
    (!contact.id.is_empty()).then_some(contact)
}

const CALENDAR_GROUPS: &str = "/v1.0/me/calendarGroups";
const CALENDAR_FIELDS: &str = "id,name,color,hexColor,canEdit,canViewPrivateItems,owner,isDefaultCalendar";
/// The same with what a calendar's meetings may be held online with, which is asked for first.
const CALENDAR_FIELDS_ONLINE: &str = "id,name,color,hexColor,canEdit,canViewPrivateItems,owner,isDefaultCalendar,allowedOnlineMeetingProviders";
const MASTER_CATEGORIES: &str = "/v1.0/me/outlook/masterCategories";
/// Most groups of calendars one account is taken to have.
const MAX_GROUPS: usize = 30;

/// The account's calendars, each with the name of the group it is listed under, its colour,
/// whether it is the main one and whether the account may change it, for the owner of `token`
/// (an access token for Graph). Calendars and groups are only read.
///
/// The groups are asked for first and then the calendars of each. Should Graph not list the
/// groups, the calendars come without them. [`CalendarsDownload::complete`] and
/// [`CalendarsDownload::for_account`] are as for [`events`].
pub fn calendars(token: &str) -> Result<CalendarsDownload, NetError> {
    calendars_at(&graph_base(), token, None)
}

/// The calendars of the shared mailbox at `address` (#70), as [`calendars`] lists the account's
/// own, for the owner of `token` (an access token for Graph that covers shared calendars):
/// asked at `/users/{address}/calendars` and never on `/me`. A shared mailbox has no groups
/// of calendars to list, so none is named. A mailbox whose calendar is shared for free and busy
/// times only doesn't list its calendars at all: that is [`NetError::Forbidden`].
pub fn shared_calendars(token: &str, address: &str) -> Result<CalendarsDownload, NetError> {
    calendars_at(&graph_base(), token, Some(address))
}

/// The mailbox's own list of categories, with a colour for each, for the owner of `token`. This
/// takes a permission of its own (`MailboxSettings.Read`): without it the answer is
/// [`NetError::Forbidden`], and nothing else is the worse for it.
pub fn categories(token: &str) -> Result<CategoriesDownload, NetError> {
    categories_at(&graph_base(), token)
}

/// When the people of `ask` are busy in its stretch, as far as the owner of `token` may know:
/// when their time is taken and how (busy, tentative, out of office), never by what. At most
/// [`MAX_SCHEDULES`] people and [`MAX_SCHEDULE_STRETCH`] seconds at once.
pub fn schedule(token: &str, ask: &FreeBusyAsk) -> Result<FreeBusy, NetError> {
    schedule_at(&graph_base(), token, ask)
}

/// Our own colours for the names Graph has for a calendar's colour, when it gives no colour as
/// such; `None` for "whatever the program likes" and for a name that isn't known.
fn calendar_color(name: &str) -> Option<Rgb> {
    const COLORS: [(&str, Rgb); 9] = [
        ("lightblue", Rgb(0x4A, 0x90, 0xD9)),
        ("lightgreen", Rgb(0x4C, 0xAF, 0x6A)),
        ("lightorange", Rgb(0xE8, 0x8F, 0x3C)),
        ("lightgray", Rgb(0x9A, 0xA0, 0xA6)),
        ("lightyellow", Rgb(0xD9, 0xB5, 0x2B)),
        ("lightteal", Rgb(0x2F, 0xA8, 0xA0)),
        ("lightpink", Rgb(0xD9, 0x6C, 0xA6)),
        ("lightbrown", Rgb(0xA6, 0x7C, 0x52)),
        ("lightred", Rgb(0xD9, 0x53, 0x4F)),
    ];
    let name = name.to_ascii_lowercase();
    COLORS.iter().find(|(known, _)| *known == name).map(|(_, color)| *color)
}

/// Our own colours for the numbered colours Graph gives a category (`preset0` to `preset24`:
/// red, orange, brown, yellow, green, teal, olive, blue, purple, cranberry, steel, dark steel,
/// gray, dark gray, black, then the dark ones of the first ten); grey for none or any other.
fn category_color(name: &str) -> Rgb {
    const COLORS: [Rgb; 25] = [
        Rgb(0xD1, 0x3B, 0x3B),
        Rgb(0xE0, 0x7B, 0x1F),
        Rgb(0x9C, 0x6B, 0x3F),
        Rgb(0xD9, 0xA4, 0x0C),
        Rgb(0x2E, 0x8B, 0x57),
        Rgb(0x1A, 0x9B, 0xA6),
        Rgb(0x7A, 0x8B, 0x2E),
        Rgb(0x2F, 0x6F, 0xD0),
        Rgb(0x7E, 0x57, 0xC2),
        Rgb(0xB0, 0x2E, 0x6B),
        Rgb(0x6E, 0x8A, 0xA5),
        Rgb(0x4A, 0x5F, 0x73),
        Rgb(0x8A, 0x8A, 0x8A),
        Rgb(0x5A, 0x5A, 0x5A),
        Rgb(0x2B, 0x2B, 0x2B),
        Rgb(0x8E, 0x1F, 0x1F),
        Rgb(0xA8, 0x52, 0x0C),
        Rgb(0x6B, 0x45, 0x23),
        Rgb(0x9A, 0x73, 0x00),
        Rgb(0x1C, 0x5E, 0x38),
        Rgb(0x0F, 0x66, 0x6E),
        Rgb(0x52, 0x5E, 0x1A),
        Rgb(0x1B, 0x45, 0x8A),
        Rgb(0x52, 0x35, 0x8A),
        Rgb(0x78, 0x1B, 0x47),
    ];
    let number = name.to_ascii_lowercase().strip_prefix("preset").and_then(|n| n.parse::<usize>().ok());
    number.and_then(|n| COLORS.get(n).copied()).unwrap_or(Rgb(0x8A, 0x8A, 0x8A))
}

/// One calendar of a list as the mailbox can take it, in the group called `group`.
fn calendar_of(entry: &Value, group: &str) -> Option<RemoteCalendar> {
    let id = text(entry, "id");
    // (An id that says it was opened by address is never one a server's list can bring: #70.)
    (!id.is_empty() && !id.starts_with(mailcraft_model::remote::OPENED_PREFIX)).then(|| RemoteCalendar {
        id: id.to_string(),
        name: text(entry, "name").to_string(),
        color: Rgb::parse(text(entry, "hexColor")).or_else(|| calendar_color(text(entry, "color"))),
        group: group.to_string(),
        rights: if entry.get("canEdit").and_then(Value::as_bool) == Some(false) { Rights::Read } else { Rights::Edit },
        owner: entry
            .get("owner")
            .filter(|owner| owner.is_object())
            .and_then(|owner| CalendarOwner { address: text(owner, "address").to_string(), name: text(owner, "name").to_string() }.cleaned()),
        main: entry.get("isDefaultCalendar").and_then(Value::as_bool).unwrap_or(false),
        // What its meetings may be held online with, when the answer says (it was asked for).
        online_providers: entry
            .get("allowedOnlineMeetingProviders")
            .and_then(Value::as_array)
            .map(|allowed| allowed.iter().take(8).filter_map(Value::as_str).filter_map(OnlineProvider::parse).collect()),
    })
}

/// The calendars of one list (`path`), each in the group called `group`, into `download`: asked
/// for with what their meetings may be held online with, and without when Graph won't have the
/// question that way (the calendars still come, and say nothing of it). Whether the list was
/// read to its end.
fn calendars_of(graph: &Graph<'_>, path: &str, group: &str, download: &mut CalendarsDownload) -> Result<bool, NetError> {
    let mut whole = Err(nonsense("the calendars weren't asked for"));
    for fields in [CALENDAR_FIELDS_ONLINE, CALENDAR_FIELDS] {
        let query = http::form(&[("$top", PAGE_SIZE), ("$select", fields)]);
        let url = graph.url(&format!("{path}?{query}"));
        whole =
            graph.list(Doing::Calendar, url, &[], MAX_REMOTE_CALENDARS, &mut |entry: &Value| download.calendars.extend(calendar_of(entry, group)));
        match &whole {
            Err(NetError::Proto(ProtoError::Rejected(why))) if why.starts_with(BAD_REQUEST) => {
                log::debug!("calendars: asked again with fewer fields")
            }
            _ => break,
        }
    }
    whole
}

/// [`calendars`] from Graph at `base`, or the calendars of the shared mailbox at `shared`.
fn calendars_at(base: &str, token: &str, shared: Option<&str>) -> Result<CalendarsDownload, NetError> {
    let graph = Graph::at(base, token)?.of_shared(shared)?;
    let mut download = CalendarsDownload { complete: true, ..Default::default() };
    if graph.shared() {
        // (One flat list: a shared mailbox's groups of calendars aren't of the owner's concern.)
        download.complete = calendars_of(&graph, &graph.mail_path("/calendars"), "", &mut download)?;
        download.tidy();
        download.complete &= !download.calendars.is_empty();
        log::debug!("calendars: {} of a shared mailbox's, complete: {}", download.calendars.len(), download.complete);
        return Ok(download);
    }
    let mut groups: Vec<(String, String)> = Vec::new();
    let query = http::form(&[("$top", PAGE_SIZE), ("$select", "id,name")]);
    let grouped = graph.list(Doing::Calendar, graph.url(&format!("{CALENDAR_GROUPS}?{query}")), &[], MAX_GROUPS, &mut |entry: &Value| {
        let id = mailcraft_model::remote::clean_key(text(entry, "id"));
        if !id.is_empty() {
            groups.push((id, text(entry, "name").to_string()));
        }
    });
    match grouped {
        Ok(whole) if !groups.is_empty() => {
            download.complete = whole;
            for (id, name) in &groups {
                let whole = calendars_of(&graph, &format!("{CALENDAR_GROUPS}/{}/calendars", http::encode(id)), name, &mut download)?;
                download.complete &= whole;
            }
        }
        // A sign-in that is no longer taken, or a Graph that is busy: nothing more is asked.
        Err(e) if e.is_refused_sign_in() || matches!(e, NetError::Busy { .. }) => return Err(e),
        // No groups to be had: the calendars as one list.
        _ => download.complete = calendars_of(&graph, CALENDARS, "", &mut download)?,
    }
    download.tidy();
    // A list with nothing in it is no reason to take every calendar away.
    download.complete &= !download.calendars.is_empty();
    log::debug!("calendars: {} of them in {} groups, complete: {}", download.calendars.len(), groups.len(), download.complete);
    Ok(download)
}

/// [`categories`] from Graph at `base`.
fn categories_at(base: &str, token: &str) -> Result<CategoriesDownload, NetError> {
    let graph = Graph::at(base, token)?;
    let mut download = CategoriesDownload::default();
    graph.list(Doing::Categories, graph.url(MASTER_CATEGORIES), &[], MAX_REMOTE_CATEGORIES, &mut |entry: &Value| {
        download.categories.push(Category { name: text(entry, "displayName").to_string(), color: category_color(text(entry, "color")) });
    })?;
    download.tidy();
    Ok(download)
}

/// [`schedule`] from Graph at `base`.
fn schedule_at(base: &str, token: &str, ask: &FreeBusyAsk) -> Result<FreeBusy, NetError> {
    schedule_with(&Graph::at(base, token)?, ask)
}

/// The longest stretch of free and busy times that is asked for one shared mailbox's calendar in
/// one go, and most such goes for one download: the window of a calendar download (a month back,
/// a year ahead) in 60-day stretches, with room to spare.
const SCHEDULE_STRETCH: Timestamp = 60 * 86_400;
const MAX_SCHEDULE_ASKS: usize = 9;

/// A shared mailbox's main calendar as its free and busy times alone (#70): what the owner of
/// the token may see of a calendar that is shared for those. Each stretch of time that is taken
/// is an event with no title and no place, only how it shows (busy, tentative, out of office,
/// working elsewhere); an event is known again by when it is and how it shows. The window is
/// asked for in stretches of [`SCHEDULE_STRETCH`]. A shared mailbox that tells nothing of its
/// time either is [`NetError::Forbidden`] again.
fn free_busy_events(graph: &Graph<'_>, address: &str, from: Timestamp, to: Timestamp) -> Result<EventsDownload, NetError> {
    let denied = || NetError::Forbidden("this account may only see when this calendar is busy, and that can't be had".into());
    let mut download = EventsDownload { from, to, complete: true, free_busy: true, ..Default::default() };
    let mut seen: HashSet<String> = HashSet::new();
    let mut at = from;
    for n in 0..MAX_SCHEDULE_ASKS {
        if at >= to {
            break;
        }
        let end = to.min(at.saturating_add(SCHEDULE_STRETCH));
        let answer = schedule_with(graph, &FreeBusyAsk { people: vec![address.to_string()], from: at, to: end })?;
        let Some(person) = answer.people.first() else { return Err(denied()) };
        if !person.error.is_empty() {
            return Err(denied());
        }
        for busy in &person.busy {
            let id = format!("freebusy:{}:{}:{}", busy.start, busy.end, busy.show_as.code());
            if busy.show_as != ShowAs::Free && seen.insert(id.clone()) && download.events.len() < MAX_LISTED_EVENTS {
                download.events.push(RemoteEvent {
                    id,
                    start: busy.start,
                    end: busy.end,
                    show_as: busy.show_as,
                    is_organizer: Some(false),
                    ..Default::default()
                });
            }
        }
        at = end;
        // Not all of the window was asked for: nothing is removed for being missing from it.
        if n + 1 == MAX_SCHEDULE_ASKS && at < to {
            download.complete = false;
        }
    }
    log::debug!("calendar: {} blocks of free and busy time from a shared mailbox", download.events.len());
    Ok(download)
}

/// [`schedule`], asked of `graph`: on `/me` for the account, on `/users/{address}` for a shared
/// mailbox's own time.
fn schedule_with(graph: &Graph<'_>, ask: &FreeBusyAsk) -> Result<FreeBusy, NetError> {
    let mut ask = ask.clone();
    ask.tidy();
    if ask.people.is_empty() {
        return Err(NetError::Config("nobody with an address was asked about".into()));
    }
    if ask.to <= ask.from || ask.to.saturating_sub(ask.from) > MAX_SCHEDULE_STRETCH {
        return Err(NetError::Config("free and busy times are asked for a stretch of 62 days at most".into()));
    }
    let body = json!({
        "schedules": ask.people,
        "startTime": date_time(ask.from, false, ""),
        "endTime": date_time(ask.to, false, ""),
        "availabilityViewInterval": 30,
    })
    .to_string();
    let headers = [("Content-Type", "application/json"), ("Prefer", "outlook.timezone=\"UTC\"")];
    let reply = graph.ask(&graph.url(&graph.mail_path("/calendar/getSchedule")), "POST", &headers, Some(&body), ASK_TIMEOUT, MAX_PAGE)?;
    if reply.status != 200 {
        return Err(graph.refusal(Doing::Calendar, &reply, &body));
    }
    let mut answer = FreeBusy { from: ask.from, to: ask.to, people: Vec::new() };
    for entry in page_of(&reply.body)?.entries.iter().take(2 * MAX_SCHEDULES) {
        // Only those who were asked about, under the address they were asked about by.
        let named = text(entry, "scheduleId").trim();
        let Some(email) = ask.people.iter().find(|email| email.eq_ignore_ascii_case(named)) else { continue };
        let items = entry.get("scheduleItems").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        // When, and how the time shows: nothing of what it is taken by is read.
        let busy = items
            .iter()
            .take(MAX_BUSY)
            .filter_map(|item| {
                Some(BusyTime { start: when(item.get("start"))?, end: when(item.get("end"))?, show_as: ShowAs::parse(text(item, "status"))? })
            })
            .collect();
        let code = entry.get("error").map_or("", |error| text(error, "responseCode"));
        let known = !code.is_empty() && code.len() <= MAX_CODE && code.bytes().all(|b| b.is_ascii_alphanumeric());
        let error = match entry.get("error").filter(|error| error.is_object()) {
            Some(_) if known => format!("their time couldn't be looked up ({code})"),
            Some(_) => "their time couldn't be looked up".to_string(),
            None => String::new(),
        };
        answer.people.push(PersonSchedule { email: email.clone(), busy, error });
    }
    answer.tidy();
    Ok(answer)
}

mod people;
pub use people::{Opened, open_calendar};

mod tasks;
pub use tasks::{push_tasks, tasks};

mod settings;
pub use settings::{MailboxSettings, WorkingHours, mailbox_settings};

mod favorites;
pub use favorites::favorites;

pub mod pace;

pub mod mail;
pub use mail::{fetch_messages, search_messages, sync_folders, sync_mail, sync_mail_renewing};

mod check;
pub use check::{CHECK_TITLE, CalendarCheck, CheckStep, STEPS as CHECK_STEPS, check_calendar, check_start, no_sign_in};

#[cfg(any(test, feature = "stand-in"))]
pub mod standin;

#[cfg(test)]
mod people_tests;
#[cfg(test)]
mod shared_calendar_tests;
#[cfg(test)]
mod shared_calendar_write_tests;
#[cfg(test)]
mod tasks_tests;

#[cfg(test)]
mod shared_calendar_move_tests;
#[cfg(test)]
mod standin_tests;
#[cfg(test)]
mod tests;
