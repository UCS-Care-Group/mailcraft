// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar, what goes from it into mail, and finding in it: some days of the owner's calendar as
//! a message (Email Calendar) or in the one being written (Insert › Calendar), a message to the
//! people of a meeting (Contact Attendees) or to its organizer (Reply by Email), who has answered
//! a meeting (Tracking), and Search Calendar.
//!
//! Nothing here sends: a message is opened for the owner to read, change and send, or the text
//! is put into the one they are writing. A summary says of each appointment no more than the
//! owner chose (`detail`), and of one marked private only when it is busy; it is of the owner's
//! own calendars alone (never of a calendar somebody shared with them), and names nobody who is
//! invited. Tracking says what is already known here of the answers; it asks no server.

use mailcraft_i18n::Lang;
use mailcraft_model::search::Query;
use mailcraft_model::{Address, Body, Event, Id, Message, Response, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::{bad, fail, needs_event, remote, target};
use crate::cmd::{base64_encode, needs_compose};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Most days a summary covers.
pub const MAX_DAYS: i64 = 366;
/// Most appointments a summary lists; past them it says how many more there are.
const MAX_ITEMS: usize = 2_000;
/// Most people a message to a meeting's people is addressed to.
const MAX_PEOPLE: usize = 500;
/// Most appointments a search answers with.
const MAX_FOUND: usize = 500;

/// The commands beyond [`LEAVING`](crate::cmd::compose::LEAVING) that start a new message, as
/// `mail.new` does, and so close the one being written: what isn't saved of it goes to Drafts
/// first, or the owner is asked, as for those (#442, #494).
pub const STARTS_MESSAGE: [&str; 3] = ["calendar.email", "calendar.contactAttendees", "calendar.respondEmail"];

/// Whether `id` is one of [`STARTS_MESSAGE`].
pub fn starts_message(id: &str) -> bool {
    STARTS_MESSAGE.contains(&id)
}

/// [`saves_first`](crate::cmd::compose::saves_first) for [`STARTS_MESSAGE`]: as for `mail.new`.
pub fn saves_first(s: &Session, id: &str, params: &Value) -> bool {
    starts_message(id) && crate::cmd::compose::saves_first(s, "mail.new", params)
}

/// The parameters of a summary, for both commands that make one.
macro_rules! range {
    () => {
        r#""from"?: "YYYY-MM-DD" (default: today), "to"?: "YYYY-MM-DD" (the last day; default: a week from "from"; 366 days at most), "detail"?: "availability" (default: when the owner is free or busy) | "limited" (and the titles) | "full" (and where, and the notes), "calendar"?: number (one of the owner's own calendars; default: those of the owner's that are shown) — an appointment marked private is said as busy or free alone, whatever "detail" says, and nobody invited to anything is named"#
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.email", "Email Calendar", "Home › Share", email)
            .pure()
            .params(concat!("{", range!(), r#", "ics"?: bool (an iCalendar file of the same goes along; default true), "discard"?: true} — starts a new message with a summary of those days in it, for the owner to address and send; nothing is sent (a message being written with changes that aren't saved is saved to Drafts first, unless `discard`)"#)),
        CommandSpec::new("insert.calendar", "Calendar", "Insert › Include", insert)
            .pure()
            .params(concat!("{", range!(), "} — puts the summary into the message being written, where the caret is"))
            .when(needs_compose),
        CommandSpec::new("calendar.contactAttendees", "Contact Attendees", "Meeting › Attendees", contact_attendees)
            .pure()
            .params(r#"{"id"?: number (default: the selected appointment), "discard"?: true} — starts a new message to its organizer and everyone invited, but the owner; nothing is sent"#)
            .selection(needs_event),
        CommandSpec::new("calendar.respondEmail", "Reply by Email", "Meeting › Respond", respond_email)
            .pure()
            .params(r#"{"id"?: number (default: the selected meeting), "all"?: bool (everyone invited too; default: the organizer alone), "discard"?: true} — starts a new message to the meeting's organizer about it; nothing is sent, and the meeting isn't answered"#)
            .selection(needs_event),
        CommandSpec::new("calendar.tracking", "Tracking", "Meeting › Show", tracking)
            .pure()
            .params(r#"{"id"?: number} (default: the selected meeting) — who is invited and how each has answered, as far as is known here: {"id", "title", "organizer", "organizes", "attendees": [{"name", "email", "optional", "response"}], "counts": {"accepted", "tentative", "declined", "none"}}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.search", "Search Calendar", "Home › Find", search)
            .pure()
            .params(r#"{"query": string (as the mail search box reads it: words, "quoted phrases", subject:, from: (the organizer), to: (who is invited), body: (where, and the notes), category:, on:, before:, after:, received: (the day it starts), importance:, and - to leave out), "all"?: bool (every calendar; default: those shown)} — the appointments that match, each once, by when they start: {"query", "events": [{"id", "title", "start", "end", "allDay", "location", "repeats", "calendar"}], "more": number}"#),
    ]
}

/// How much of each appointment a summary says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// When the owner is free or busy.
    Availability,
    /// And the titles.
    Limited,
    /// And where, and the notes.
    Full,
}

impl Detail {
    pub fn parse(s: &str) -> Option<Detail> {
        match s {
            "availability" => Some(Detail::Availability),
            "limited" => Some(Detail::Limited),
            "full" => Some(Detail::Full),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Detail::Availability => "availability",
            Detail::Limited => "limited",
            Detail::Full => "full",
        }
    }
}

/// One appointment of a summary, as much of it as is to be said.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    start: DateTime,
    end: DateTime,
    all_day: bool,
    show_as: ShowAs,
    /// Empty when only when it is is said.
    title: String,
    location: String,
    notes: String,
}

/// Some days of the owner's calendar, as a summary says them.
#[derive(Clone, Debug)]
pub struct Summary {
    from: Date,
    to: Date,
    detail: Detail,
    /// The one calendar named, when one was.
    name: Option<String>,
    items: Vec<Item>,
    /// Appointments past [`MAX_ITEMS`].
    more: usize,
}

/// Text from an appointment, for one line of a message: no control or format characters, and
/// not too long.
fn one_line(text: &str, max: usize) -> String {
    mailcraft_names::shown(text, max)
}

/// The notes of an appointment, for a few lines of a message.
fn notes_of(text: &str) -> String {
    text.lines().map(|l| one_line(l, 200)).filter(|l| !l.is_empty()).take(12).collect::<Vec<_>>().join("\n")
}

fn day_said(lang: Lang, d: Date) -> String {
    let day = mailcraft_i18n::date::Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() };
    mailcraft_i18n::date::long(&lang, day)
}

/// The owner's own calendars that a summary is of: the one `calendar` names (refused when it is
/// somebody else's), else those of the owner's that are shown.
fn calendars(s: &Session, v: &Value) -> Result<(Vec<Id>, Option<String>), CmdError> {
    match p::id_strict(v, "calendar")? {
        Some(id) => {
            let c = s.mailbox.calendar(id).ok_or_else(|| failed!("no calendar with id {id}", id))?;
            if !remote::is_own_calendar(&s.mailbox, c) {
                return Err(fail("only your own calendars can be put in a message"));
            }
            Ok((vec![id], Some(one_line(&c.name, 120))))
        }
        None => Ok((s.mailbox.calendars.iter().filter(|c| c.visible && remote::is_own_calendar(&s.mailbox, c)).map(|c| c.id).collect(), None)),
    }
}

impl Summary {
    /// The summary `v` asks for ([`RANGE`]).
    pub fn read(s: &Session, v: &Value) -> Result<Summary, CmdError> {
        let from = p::date(v, "from")?.unwrap_or_else(|| s.today());
        let to = p::date(v, "to")?.unwrap_or_else(|| from.add_days(6));
        if to < from {
            return Err(bad("`to` is before `from`"));
        }
        if from.days_until(to) >= MAX_DAYS {
            return Err(bad("a summary covers a year at most"));
        }
        let detail = match v.get("detail") {
            None | Some(Value::Null) => Detail::Availability,
            Some(d) => d.as_str().and_then(Detail::parse).ok_or_else(|| bad("`detail` is availability, limited or full"))?,
        };
        let (shown, name) = calendars(s, v)?;
        let here = remote::offset_of(s);
        let mut occurrences: Vec<(mailcraft_model::Occurrence, &Event)> = s
            .mailbox
            .events
            .values()
            .filter(|e| shown.contains(&e.calendar) && !e.cancelled)
            .flat_map(|e| e.occurrences_in(from, to, &here).into_iter().map(move |o| (o, e.as_ref())))
            // (An appointment that ends as the first day starts is not on it.)
            .filter(|(o, _)| o.end > DateTime::at_midnight(from) || o.start >= DateTime::at_midnight(from))
            .collect();
        occurrences.sort_by_key(|(o, e)| (o.start, o.end, e.id));
        let more = occurrences.len().saturating_sub(MAX_ITEMS);
        let items = occurrences.into_iter().take(MAX_ITEMS).map(|(o, e)| item(e, o.start, o.end, detail)).collect();
        Ok(Summary { from, to, detail, name, items, more })
    }

    /// How many appointments it lists.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Its subject, for a message of its own: `Calendar: Monday, November 2, 2026 – …`.
    pub fn subject(&self, lang: Lang) -> String {
        let what = self.name.clone().unwrap_or_else(|| lang.t("Calendar").to_string());
        format!("{what}: {}", self.span(lang))
    }

    fn span(&self, lang: Lang) -> String {
        if self.from == self.to { day_said(lang, self.from) } else { format!("{} – {}", day_said(lang, self.from), day_said(lang, self.to)) }
    }

    /// The summary as text: the days, and under each what is on it.
    pub fn text(&self, s: &Session) -> String {
        let lang = s.interface_language;
        let twelve = s.prefs.twelve_hour;
        let clock = |at: DateTime| mailcraft_i18n::date::time(&lang, at.hour, at.minute, twelve);
        let mut out = String::new();
        out.push_str(&self.subject(lang));
        out.push('\n');
        let zone = s.zone();
        if !zone.is_local() {
            out.push_str(&format!("{} {}\n", lang.t("Time zone:"), one_line(&zone.id(), 80)));
        }
        if self.items.is_empty() {
            out.push('\n');
            out.push_str(lang.t("Nothing is scheduled on these days."));
            out.push('\n');
            return out;
        }
        let mut day: Option<Date> = None;
        for item in &self.items {
            // Under the day it starts on; one that started earlier, under the first day.
            let on = item.start.date.max(self.from);
            if day != Some(on) {
                out.push('\n');
                out.push_str(&day_said(lang, on));
                out.push('\n');
                day = Some(on);
            }
            let when = if item.all_day {
                let last = item.end.add_minutes(-1).date.max(item.start.date);
                if last > item.start.date { format!("{} – {}", lang.t("All day"), day_said(lang, last)) } else { lang.t("All day").to_string() }
            } else if item.end.date != item.start.date {
                format!("{} – {} {}", clock(item.start), day_said(lang, item.end.date), clock(item.end))
            } else {
                format!("{} – {}", clock(item.start), clock(item.end))
            };
            let busy = lang.t(item.show_as.label());
            let what = if item.title.is_empty() { busy.to_string() } else { format!("{busy}: {}", item.title) };
            out.push_str(&format!("    {when}  {what}"));
            if !item.location.is_empty() {
                out.push_str(&format!(" ({})", item.location));
            }
            out.push('\n');
            for line in item.notes.lines() {
                out.push_str(&format!("        {line}\n"));
            }
        }
        if self.more > 0 {
            out.push_str(&format!("\n{} {}\n", lang.t("Appointments not listed:"), self.more));
        }
        out
    }

    /// The same as an iCalendar file: each appointment once, as it is on its day (an occurrence
    /// of a series is an appointment of its own), with no more of it than the text says.
    pub fn ics(&self, s: &Session) -> String {
        let events: Vec<Event> = self
            .items
            .iter()
            .enumerate()
            .map(|(n, item)| Event {
                id: n as Id + 1,
                uid: format!("mailcraft-shared-{}-{}@mailcraft.local", item.start.iso().replace([':', '-'], ""), n + 1),
                title: if item.title.is_empty() { item.show_as.label().to_string() } else { item.title.clone() },
                location: item.location.clone(),
                description: item.notes.clone(),
                start: item.start,
                end: item.end,
                all_day: item.all_day,
                show_as: item.show_as,
                ..Default::default()
            })
            .collect();
        let refs: Vec<&Event> = events.iter().collect();
        let opts = mailcraft_ical::WriteOptions { method: "PUBLISH".into(), now: s.now() };
        mailcraft_ical::write_in(&refs, &[], &opts, Some(s.zone()))
    }

    pub fn detail(&self) -> Detail {
        self.detail
    }
}

/// What a summary says of one occurrence of `e`.
fn item(e: &Event, start: DateTime, end: DateTime, detail: Detail) -> Item {
    let detail = if e.private { Detail::Availability } else { detail };
    let (title, location, notes) = match detail {
        Detail::Availability => (String::new(), String::new(), String::new()),
        Detail::Limited => (one_line(&e.title, 200), String::new(), String::new()),
        Detail::Full => (one_line(&e.title, 200), one_line(&e.location, 200), notes_of(&e.description)),
    };
    Item { start, end, all_day: e.all_day, show_as: e.show_as, title, location, notes }
}

/// Email Calendar: a new message with the summary in it, and the same as a file.
fn email(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["from", "to", "detail", "calendar", "ics", "discard"])?;
    let summary = Summary::read(s, v)?;
    let attach = match v.get("ics") {
        None | Some(Value::Null) => true,
        Some(b) => b.as_bool().ok_or_else(|| bad("`ics` is true or false"))?,
    };
    let lang = s.interface_language;
    let text = summary.text(s);
    let ics = attach.then(|| summary.ics(s));
    s.run("mail.new", &json!({"subject": summary.subject(lang), "body": text, "discard": p::bool(v, "discard") == Some(true)}))?;
    if let Some(ics) = ics {
        s.run("compose.attach", &json!({"name": "calendar.ics", "data": base64_encode(ics.as_bytes()), "mime": "text/calendar"}))?;
    }
    Ok(json!({"composing": true, "events": summary.len(), "detail": summary.detail().code(), "ics": attach}))
}

/// Insert › Calendar: the summary, where the caret is in the message being written.
fn insert(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["from", "to", "detail", "calendar"])?;
    let summary = Summary::read(s, v)?;
    let text = summary.text(s);
    s.run("compose.type", &json!({"text": text}))?;
    Ok(json!({"inserted": true, "events": summary.len(), "detail": summary.detail().code()}))
}

/// Whether `a` is one of the owner's own addresses.
fn own(s: &Session, a: &Address) -> bool {
    s.mailbox.accounts.iter().any(|x| x.email.trim().eq_ignore_ascii_case(a.email.trim()))
}

/// Each of `people` once, whose address is one and isn't the owner's, at most [`MAX_PEOPLE`].
fn others<'a>(s: &Session, people: impl Iterator<Item = &'a Address>) -> Vec<Address> {
    let mut out: Vec<Address> = Vec::new();
    for a in people {
        if out.len() >= MAX_PEOPLE {
            break;
        }
        if a.is_valid() && !own(s, a) && !out.iter().any(|x| x.same_mailbox(a)) {
            out.push(Address::new(one_line(&a.name, 120), a.email.trim()));
        }
    }
    out
}

/// The account a message about `e` goes from: the one whose server keeps it, else as a new
/// message would.
fn account_of(s: &Session, e: &Event) -> Option<Id> {
    remote::keeper(&s.mailbox, &e.origin).map(|a| a.id)
}

/// A new message about `e` to `to`, with `subject`.
fn message_about(s: &mut Session, v: &Value, e: &Event, to: &[Address], subject: String) -> CmdResult {
    let line = to.iter().map(Address::full).collect::<Vec<_>>().join("; ");
    let mut params = json!({"to": line, "subject": subject, "discard": p::bool(v, "discard") == Some(true)});
    if let (Some(account), Some(fields)) = (account_of(s, e), params.as_object_mut()) {
        fields.insert("account".into(), json!(account));
    }
    s.run("mail.new", &params)?;
    Ok(json!({"composing": true, "to": to.len()}))
}

fn contact_attendees(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "discard"])?;
    let id = target(s, v)?;
    let e = s.mailbox.event(id).cloned().ok_or_else(|| fail("the event is gone"))?;
    let to = others(s, e.organizer.iter().chain(e.attendees.iter().map(|a| &a.address)));
    if to.is_empty() {
        return Err(fail("nobody else is invited to this appointment"));
    }
    message_about(s, v, &e, &to, one_line(&e.title, 200))
}

/// `RE: title`, unless it says so already.
fn re(title: &str) -> String {
    let title = one_line(title, 200);
    if title.to_ascii_lowercase().starts_with("re:") { title } else { format!("RE: {title}") }
}

fn respond_email(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "all", "discard"])?;
    let id = target(s, v)?;
    let e = s.mailbox.event(id).cloned().ok_or_else(|| fail("the event is gone"))?;
    let all = match v.get("all") {
        None | Some(Value::Null) => false,
        Some(b) => b.as_bool().ok_or_else(|| bad("`all` is true or false"))?,
    };
    let organizer = e.organizer.as_ref().filter(|o| o.is_valid() && !own(s, o)).ok_or_else(|| fail("this meeting has no organizer to write to"))?;
    let rest = e.attendees.iter().map(|a| &a.address).filter(|_| all);
    let to = others(s, std::iter::once(organizer).chain(rest));
    message_about(s, v, &e, &to, re(&e.title))
}

fn response_code(r: Response) -> &'static str {
    match r {
        Response::None => "none",
        Response::Accepted => "accepted",
        Response::Tentative => "tentative",
        Response::Declined => "declined",
    }
}

fn tracking(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id"])?;
    let id = target(s, v)?;
    let e = s.mailbox.event(id).ok_or_else(|| fail("the event is gone"))?;
    let mut counts = [0usize; 4];
    let attendees: Vec<Value> = e
        .attendees
        .iter()
        .take(MAX_PEOPLE)
        .map(|a| {
            let n = match a.response {
                Response::Accepted => 0,
                Response::Tentative => 1,
                Response::Declined => 2,
                Response::None => 3,
            };
            if let Some(c) = counts.get_mut(n) {
                *c = c.saturating_add(1);
            }
            json!({"name": one_line(&a.address.name, 120), "email": one_line(&a.address.email, 320), "optional": a.optional, "response": response_code(a.response)})
        })
        .collect();
    let [accepted, tentative, declined, none] = counts;
    Ok(json!({
        "id": e.id,
        "title": one_line(&e.title, 200),
        "organizer": e.organizer.as_ref().map(|o| json!({"name": one_line(&o.name, 120), "email": one_line(&o.email, 320)})),
        "organizes": !e.origin.guest,
        "attendees": attendees,
        "counts": {"accepted": accepted, "tentative": tentative, "declined": declined, "none": none},
    }))
}

/// An appointment as a message, for the search box's language to read: its title the subject,
/// its organizer the sender, who is invited the recipients (optional ones on Cc), where it is and
/// its notes the text, and the day it starts the day it came.
fn as_message(e: &Event, at: mailcraft_time::Timestamp) -> Message {
    let (optional, required): (Vec<_>, Vec<_>) = e.attendees.iter().partition(|a| a.optional);
    Message {
        subject: e.title.clone(),
        from: e.organizer.clone().unwrap_or_default(),
        to: required.into_iter().map(|a| a.address.clone()).collect(),
        cc: optional.into_iter().map(|a| a.address.clone()).collect(),
        date: at,
        received: at,
        body: Body { text: format!("{}\n{}", e.location, e.description), html: None },
        categories: e.categories.clone(),
        importance: e.importance,
        read: true,
        ..Default::default()
    }
}

fn search(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["query", "all"])?;
    let text: String = p::req_str(v, "query")?.chars().filter(|c| !c.is_control()).take(500).collect();
    let query = Query::parse(&text).on(s.today(), s.prefs.week_start);
    if query.is_empty() {
        return Err(bad("`query` has nothing to look for"));
    }
    let all = p::bool(v, "all") == Some(true);
    let here = remote::offset_of(s);
    let shown: Vec<Id> = s.mailbox.calendars.iter().filter(|c| all || c.visible).map(|c| c.id).collect();
    let mut found: Vec<&Event> = s
        .mailbox
        .events
        .values()
        .map(|e| e.as_ref())
        .filter(|e| shown.contains(&e.calendar))
        .filter(|e| query.matches_in(&as_message(e, e.start.to_timestamp(here(s.now()))), &here))
        .collect();
    found.sort_by_key(|e| (e.start, e.id));
    let more = found.len().saturating_sub(MAX_FOUND);
    let events: Vec<Value> = found
        .into_iter()
        .take(MAX_FOUND)
        .map(|e| {
            json!({"id": e.id, "title": e.title, "start": e.start.iso(), "end": e.end.iso(), "allDay": e.all_day, "location": e.location, "repeats": e.recurrence.is_some(), "calendar": e.calendar})
        })
        .collect();
    Ok(json!({"query": text, "events": events, "more": more}))
}
