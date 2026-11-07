// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A to-do made from an appointment or a contact, as Outlook makes one when either is dragged
//! onto its tasks (#831): the task's subject is the appointment's subject or the contact's name,
//! and its notes carry what the item says (when and where, the people and the description; the
//! contact's addresses, numbers and notes). The task has no link back to the item: a to-do links
//! only to a flagged message (`Task::message`), so the item's words are copied into it.
//!
//! What an appointment or a contact says is data: it is copied as text, never acted on.

use mailcraft_i18n::Lang;
use mailcraft_model::{Contact, Event, Id, Task};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::{new_task_place, put, reveal};
use crate::cmd::fail;
use crate::{CmdError, CmdResult, Module, Session, p};

/// Most of the item's own words (an appointment's description, a contact's notes) the task
/// carries, in characters, and most lines of them.
const MAX_TEXT: usize = 8_000;
const MAX_LINES: usize = 200;
/// Most of a list (attendees, addresses, numbers) the notes name.
const MAX_LISTED: usize = 50;

pub(super) fn needs_event(s: &Session) -> Option<&'static str> {
    if s.sel.event.is_none() { Some("select an appointment first") } else { None }
}

pub(super) fn needs_contact(s: &Session) -> Option<&'static str> {
    if s.sel.contact.is_none() { Some("select a contact first") } else { None }
}

/// The appointment `v` names (`id`, else the selected one) and the occurrence it means: `day`
/// for one of a series (else the selected occurrence's day), when it starts and ends.
fn named_event(s: &Session, v: &Value) -> Result<(Event, DateTime, DateTime), CmdError> {
    let named = p::id_strict(v, "id")?;
    let id = named.or(s.sel.event).ok_or_else(|| fail("select an appointment first"))?;
    let ev = s.mailbox.event(id).cloned().ok_or_else(|| failed!("no appointment with id {id}", id))?;
    let day = match p::date(v, "day")? {
        Some(day) => Some(day),
        // The selected occurrence, when the selected appointment is the one meant.
        None if named.is_none() || named == s.sel.event => s.sel.event_day,
        None => None,
    };
    let Some(day) = day.filter(|_| ev.recurrence.is_some()) else { return Ok((ev.clone(), ev.start, ev.end)) };
    let here = crate::cmd::remote::offset_of(s);
    let occurrence = ev.occurrences_in(day.add_days(-1), day.add_days(1), &here).into_iter().find(|o| o.day == day);
    let Some(o) = occurrence else { return Err(failed!("the series doesn't happen on {date}", date = day.iso())) };
    Ok((ev, o.start, o.end))
}

/// The contact `v` names (`id`, else the selected one).
fn named_contact(s: &Session, v: &Value) -> Result<Contact, CmdError> {
    let id = p::id_strict(v, "id")?.or(s.sel.contact).ok_or_else(|| fail("select a contact first"))?;
    s.mailbox.contact(id).cloned().ok_or_else(|| failed!("no contact with id {id}", id))
}

/// Whether `v` names an item that is there (and a day it happens on), for the agent gate: the
/// command's own error when it doesn't.
pub(super) fn check(s: &Session, id: &str, v: &Value) -> Result<(), CmdError> {
    match id {
        "tasks.fromEvent" => named_event(s, v).map(|_| ()),
        _ => named_contact(s, v).map(|_| ()),
    }
}

fn day_said(lang: Lang, d: Date) -> String {
    let day = mailcraft_i18n::date::Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() };
    mailcraft_i18n::date::long(&lang, day)
}

fn time_said(lang: Lang, at: DateTime, twelve: bool) -> String {
    mailcraft_i18n::date::time(&lang, at.hour, at.minute, twelve)
}

/// One line of an item's text: its controls and line breaks gone, within `max` characters.
fn one(text: &str, max: usize) -> String {
    mailcraft_names::shown(text, max)
}

/// The item's own words, line by line, within the caps.
fn body(out: &mut String, text: &str) {
    let mut room = MAX_TEXT;
    let lines: Vec<String> = text
        .lines()
        .take(MAX_LINES)
        .map_while(|line| {
            if room == 0 {
                return None;
            }
            // Each line as a name is shown: no controls, nothing that turns or hides the text.
            let line = mailcraft_names::shown(line, room);
            room = room.saturating_sub(line.chars().count().max(1));
            Some(line)
        })
        .collect();
    if lines.iter().any(|l| !l.trim().is_empty()) {
        out.push('\n');
        for line in lines {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
}

/// When an occurrence is, in the interface language: its day (and last day), and its hours.
fn when(lang: Lang, ev: &Event, start: DateTime, end: DateTime, twelve: bool) -> String {
    if ev.all_day {
        // (The end of a whole-day appointment is the midnight after its last day.)
        let last = end.add_minutes(-1).date.max(start.date);
        return if last == start.date {
            format!("{} ({})", day_said(lang, start.date), lang.t("All day"))
        } else {
            format!("{} – {} ({})", day_said(lang, start.date), day_said(lang, last), lang.t("All day"))
        };
    }
    if end.date == start.date {
        format!("{} {}–{}", day_said(lang, start.date), time_said(lang, start, twelve), time_said(lang, end, twelve))
    } else {
        format!("{} {} – {} {}", day_said(lang, start.date), time_said(lang, start, twelve), day_said(lang, end.date), time_said(lang, end, twelve))
    }
}

/// The task's notes for an appointment: when and where, who organizes it and who is invited,
/// then its description.
pub(crate) fn event_notes(s: &Session, ev: &Event, start: DateTime, end: DateTime) -> String {
    let lang = s.interface_language;
    let mut out = String::new();
    out.push_str(&format!("{} {}\n", lang.t("When:"), when(lang, ev, start, end, s.prefs.twelve_hour)));
    if !ev.location.trim().is_empty() {
        out.push_str(&format!("{} {}\n", lang.t("Where:"), one(&ev.location, 300)));
    }
    if let Some(organizer) = &ev.organizer
        && !ev.attendees.is_empty()
    {
        out.push_str(&format!("{} {}\n", lang.t("Organizer:"), one(&organizer.full(), 300)));
    }
    if !ev.attendees.is_empty() {
        let names: Vec<String> = ev.attendees.iter().take(MAX_LISTED).map(|a| one(&a.address.full(), 300)).collect();
        out.push_str(&format!("{} {}\n", lang.t("Attendees:"), names.join("; ")));
    }
    if let Some(link) = ev.join_link() {
        out.push_str(&format!("{} {}\n", lang.t("Join:"), one(link, 2_000)));
    }
    body(&mut out, &ev.description);
    out
}

/// The task's notes for a contact: their job and company, addresses, numbers and sites, then
/// their notes.
pub(crate) fn contact_notes(s: &Session, c: &Contact) -> String {
    let lang = s.interface_language;
    let mut out = String::new();
    out.push_str(&one(&c.name(), 300));
    out.push('\n');
    let job: Vec<&str> = [c.title.trim(), c.department.trim(), c.company.trim()].into_iter().filter(|x| !x.is_empty()).collect();
    if !job.is_empty() {
        out.push_str(&one(&job.join(", "), 300));
        out.push('\n');
    }
    let mut listed = |label: &str, values: Vec<String>| {
        let values: Vec<String> = values.into_iter().filter(|x| !x.trim().is_empty()).take(MAX_LISTED).collect();
        if !values.is_empty() {
            out.push_str(&format!("{label} {}\n", values.join("; ")));
        }
    };
    listed(lang.t("Email:"), c.emails.iter().map(|e| one(&e.value, 300)).collect());
    let phone = |x: &mailcraft_model::Labeled| {
        let (label, value) = (one(&x.label, 60), one(&x.value, 100));
        if label.is_empty() { value } else { format!("{value} ({label})") }
    };
    listed(lang.t("Phone:"), c.phones.iter().map(phone).collect());
    listed(lang.t("Address:"), c.addresses.iter().map(|a| one(&a.one_line(), 300)).collect());
    listed(lang.t("Web page:"), c.websites.iter().map(|w| one(w, 2_000)).collect());
    if let Some(birthday) = c.birthday {
        out.push_str(&format!("{} {}\n", lang.t("Birthday:"), day_said(lang, birthday)));
    }
    body(&mut out, &c.notes);
    out
}

/// A title for the task, as `tasks.new` keeps one: one line, at most 500 characters.
fn title_of(text: &str, otherwise: &str) -> String {
    let title = mailcraft_names::shown(&text.replace(['\r', '\n'], " "), 500);
    if title.is_empty() { otherwise.to_string() } else { title }
}

/// Make the task, where a new one goes, and show it unless `show` is false (an item dropped on
/// the To-Do Bar, which leaves the module in front).
fn make(s: &mut Session, v: &Value, mut task: Task) -> CmdResult {
    task.due = p::date(v, "due")?;
    let show = p::opt_bool(v, "show")?.unwrap_or(true);
    if let Some(place) = new_task_place(s) {
        put(&mut task, place);
    }
    reveal(s, &task.origin);
    let (account, kept_at) = (task.origin.account, super::kept_at(&s.mailbox, &task.origin));
    let id: Id = s.mailbox.add_task(task);
    s.sel.task = Some(id);
    if show {
        s.view.module = Module::Tasks;
    }
    Ok(json!({"id": id, "account": account, "keptAt": kept_at}))
}

pub(super) fn from_event(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "day", "due", "show"])?;
    let (ev, start, end) = named_event(s, v)?;
    let title = title_of(&ev.title, s.interface_language.t("(no title)"));
    let notes = event_notes(s, &ev, start, end);
    make(s, v, Task { title, notes, importance: ev.importance, ..Default::default() })
}

pub(super) fn from_contact(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "due", "show"])?;
    let c = named_contact(s, v)?;
    let title = title_of(&c.name(), s.interface_language.t("(no name)"));
    let notes = contact_notes(s, &c);
    make(s, v, Task { title, notes, ..Default::default() })
}
