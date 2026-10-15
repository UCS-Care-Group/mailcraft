//! File › Print: a message in Memo Style, the calendar (Daily, Work Week, Weekly, Monthly),
//! contacts as cards and tasks as a table, laid out by `mailcraft-print` and written as a PDF.
//!
//! A programmatic call writes the PDF to `path` (or answers its bytes); it never opens a dialog.
//! The window's Print page shows the same layout as a preview ([`document`]) and hands the PDF to
//! the system to print. Nothing in a message is fetched to print it: a picture on the web stays a
//! placeholder, and only pictures the message carries itself are printed.

use std::sync::Arc;

use mailcraft_html::{Block, BlockKind};
use mailcraft_model::{Importance, Message};
use mailcraft_print::calendar::{CalendarPrint, Entry, Style as CalStyle};
use mailcraft_print::list::{Card, Column};
use mailcraft_print::memo::Memo;
use mailcraft_print::{Document, Paper, Rgb};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::{bad, deliver, fail, file_name};
use crate::{CalView, CmdError, CmdResult, CommandSpec, Module, Session, TaskFilter, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("file.print", "Print", "File › Print", print).pure().key("Mod+P").params(
        r#"{"what"?: "message"|"calendar"|"contacts"|"tasks" (default: what the module in front shows), "id"?: number (the message; default the selected one), "style"?: "memo" (a message) | "day"|"workWeek"|"week"|"month" (the calendar; default its view) | "card" (contacts) | "table" (tasks), "range"?: {"from"?: "YYYY-MM-DD", "to"?: "YYYY-MM-DD"} (the calendar's days; default the days in view, up to 400), "paper"?: "letter"|"a4", "path"?: string} — lays the print out and writes it as a PDF to `path`, or answers its bytes as base64 `data`; never opens a dialog. Pictures on the web are never fetched for it. Answers {"what", "style", "pages", "path"|"data", "bytes"}"#,
    )]
}

/// What is printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Message,
    Calendar,
    Contacts,
    Tasks,
}

impl What {
    pub fn parse(code: &str) -> Option<What> {
        match code.trim().to_ascii_lowercase().as_str() {
            "message" | "mail" | "memo" => Some(What::Message),
            "calendar" => Some(What::Calendar),
            "contacts" | "people" => Some(What::Contacts),
            "tasks" | "todo" => Some(What::Tasks),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            What::Message => "message",
            What::Calendar => "calendar",
            What::Contacts => "contacts",
            What::Tasks => "tasks",
        }
    }

    /// What the module in front prints.
    pub fn of(module: Module) -> What {
        match module {
            Module::Mail => What::Message,
            Module::Calendar => What::Calendar,
            Module::People => What::Contacts,
            Module::Tasks => What::Tasks,
        }
    }
}

/// What `params` ask to print, or what the window shows.
pub fn what(s: &Session, v: &Value) -> Result<What, CmdError> {
    match v.get("what") {
        None | Some(Value::Null) => Ok(What::of(s.view.module)),
        Some(Value::String(code)) => What::parse(code).ok_or_else(|| bad(format!("can't print \"{code}\": a message, calendar, contacts or tasks"))),
        Some(_) => Err(bad("`what` is \"message\", \"calendar\", \"contacts\" or \"tasks\"")),
    }
}

fn paper(v: &Value) -> Result<Paper, CmdError> {
    match v.get("paper") {
        None | Some(Value::Null) => Ok(Paper::default()),
        Some(Value::String(code)) => Paper::parse(code).ok_or_else(|| bad(format!("no paper \"{code}\": letter or a4"))),
        Some(_) => Err(bad("`paper` is \"letter\" or \"a4\"")),
    }
}

/// The calendar style `params` ask for, else the calendar's view.
pub fn calendar_style(s: &Session, v: &Value) -> Result<CalStyle, CmdError> {
    match p::str(v, "style") {
        Some(code) => CalStyle::parse(code).ok_or_else(|| bad(format!("no calendar style \"{code}\": day, workWeek, week or month"))),
        None if v.get("style").is_some_and(|s| !s.is_null()) => Err(bad("`style` is a string")),
        None => Ok(match s.view.calendar {
            CalView::Day => CalStyle::Day,
            CalView::WorkWeek => CalStyle::WorkWeek,
            CalView::Week => CalStyle::Week,
            CalView::Month => CalStyle::Month,
        }),
    }
}

/// The print as pages: what the Print page previews and what `file.print` writes.
pub fn document(s: &Session, v: &Value) -> Result<Document, CmdError> {
    if !v.is_object() && !v.is_null() {
        return Err(bad("the params are an object"));
    }
    let paper = paper(v)?;
    match what(s, v)? {
        What::Message => {
            if let Some(style) = p::str(v, "style")
                && !style.eq_ignore_ascii_case("memo")
            {
                return Err(bad(format!("a message prints in the \"memo\" style, not \"{style}\"")));
            }
            let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message to print"))?;
            if s.mailbox.message(id).is_none() {
                return Err(fail(format!("no message with id {id}")));
            }
            // Whole: the pictures its HTML shows are printed with it.
            let m = s.loaded_message(id)?;
            Ok(mailcraft_print::memo::layout(&memo(s, &m), paper))
        }
        What::Calendar => Ok(mailcraft_print::calendar::layout(&calendar(s, v)?, paper)),
        What::Contacts => {
            let ids = match p::ids(v) {
                Some(ids) => ids,
                None => super::people::sorted(s, ""),
            };
            let cards: Vec<Card> = ids.iter().filter_map(|id| s.mailbox.contact(*id)).map(card).collect();
            Ok(mailcraft_print::list::cards(&owner(s, None), "Contacts", &cards, paper))
        }
        What::Tasks => Ok(tasks(s, paper)),
    }
}

/// The name at the top of the pages: the owner of the message's account, else of the first.
fn owner(s: &Session, account: Option<mailcraft_model::Id>) -> String {
    let a = account.and_then(|id| s.mailbox.account(id)).or_else(|| s.mailbox.accounts.first());
    a.map(|a| if a.name.trim().is_empty() { a.email.clone() } else { a.name.clone() }).unwrap_or_default()
}

fn addresses(list: &[mailcraft_model::Address]) -> String {
    list.iter().take(500).map(|a| a.full()).collect::<Vec<_>>().join("; ")
}

/// The message's display blocks, chosen as the reading pane chooses them.
pub fn blocks(m: &Message) -> Vec<Block> {
    match m.body.html.as_deref().map(mailcraft_html::parse) {
        Some(b) if mailcraft_html::has_text(&b) || m.body.text.trim().is_empty() || !mailcraft_html::images(&b).is_empty() => b,
        _ => mailcraft_html::text_blocks(&m.body.text),
    }
}

/// The attachment a `cid:` address names: by its Content-ID as written, then without regard to
/// letter case.
fn embedded<'a>(m: &'a Message, src: &str) -> Option<&'a mailcraft_model::Attachment> {
    let src = src.trim();
    let id = src.get(..4).filter(|scheme| scheme.eq_ignore_ascii_case("cid:")).and_then(|_| src.get(4..))?;
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    if id.is_empty() {
        return None;
    }
    let parts = || m.attachments.iter().filter(|a| !a.content_id.is_empty());
    parts().find(|a| a.content_id == id).or_else(|| parts().find(|a| a.content_id.eq_ignore_ascii_case(id)))
}

fn memo(s: &Session, m: &Message) -> Memo {
    let blocks = blocks(m);
    let mut pictures: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
    for b in mailcraft_html::images(&blocks) {
        if let BlockKind::Image { src, .. } = &b.kind
            && mailcraft_html::is_embedded(src)
            && !pictures.iter().any(|(k, _)| k == src.trim())
            && let Some(a) = embedded(m, src)
        {
            pictures.push((src.trim().to_string(), a.data.clone()));
        }
    }
    let sent = DateTime::from_timestamp(m.date, s.offset_at(m.date));
    Memo {
        owner: owner(s, Some(m.account)),
        from: m.from.full(),
        sent: format!("{} {}", sent.date.long(), sent.time_short(s.prefs.twelve_hour)),
        to: addresses(&m.to),
        cc: addresses(&m.cc),
        subject: m.subject.clone(),
        attachments: m.listed_attachments().map(|(_, a)| a.name.clone()).collect(),
        importance: match m.importance {
            Importance::High => "High".into(),
            Importance::Low => "Low".into(),
            Importance::Normal => String::new(),
        },
        blocks,
        pictures,
    }
}

/// The days a calendar print covers: what `range` says, else the days in view (a month style
/// prints the month in view).
fn range(s: &Session, v: &Value, style: CalStyle) -> Result<(Date, Date), CmdError> {
    let r = match v.get("range") {
        None | Some(Value::Null) => &Value::Null,
        Some(r @ Value::Object(_)) => r,
        Some(_) => return Err(bad("`range` is {\"from\": \"YYYY-MM-DD\", \"to\": \"YYYY-MM-DD\"}")),
    };
    let (view_from, view_to) = match style {
        CalStyle::Month => (s.view.date.start_of_month(), s.view.date.end_of_month()),
        _ => super::view::calendar_range(s),
    };
    let from = p::date(r, "from")?.or(p::date(v, "from")?).unwrap_or(view_from);
    let to = p::date(r, "to")?.or(p::date(v, "to")?).unwrap_or(if from == view_from { view_to } else { from });
    if to < from {
        return Err(bad("the range ends before it starts"));
    }
    Ok((from, to.min(from.add_days(mailcraft_print::calendar::MAX_DAYS))))
}

fn rgb(c: mailcraft_model::Rgb) -> Rgb {
    Rgb(c.0, c.1, c.2)
}

fn calendar(s: &Session, v: &Value) -> Result<CalendarPrint, CmdError> {
    let style = calendar_style(s, v)?;
    let (from, to) = range(s, v, style)?;
    // A week style starts its first page at the start of that week; a month style its month.
    let (first, last) = match style {
        CalStyle::Day => (from, to),
        CalStyle::WorkWeek | CalStyle::Week => (from.start_of_week(s.prefs.week_start), to.start_of_week(s.prefs.week_start).add_days(6)),
        CalStyle::Month => (from.start_of_month().start_of_week(s.prefs.week_start), to.end_of_month().add_days(7)),
    };
    let hours = super::calendar_settings::work_hours(s);
    let entries = super::calendar::occurrences(s, first, last)
        .into_iter()
        .take(mailcraft_print::calendar::MAX_ITEMS)
        .filter_map(|o| {
            let e = s.mailbox.event(o.event)?;
            let color = s.mailbox.calendar(e.calendar).map_or(Rgb(90, 130, 190), |c| rgb(c.shown_color()));
            Some(Entry { title: e.title.clone(), location: e.location.clone(), start: o.start, end: o.end, all_day: e.all_day, color })
        })
        .collect();
    Ok(CalendarPrint {
        owner: owner(s, None),
        style,
        from,
        to,
        week_start: s.prefs.week_start,
        work_days: hours.days.clone(),
        work_start: (hours.start / 60).clamp(0, 23) as u8,
        work_end: (hours.end.saturating_add(59) / 60).clamp(1, 24) as u8,
        twelve_hour: s.prefs.twelve_hour,
        entries,
    })
}

fn card(c: &mailcraft_model::Contact) -> Card {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut add = |label: &str, value: &str| {
        if !value.trim().is_empty() {
            fields.push((label.to_string(), value.trim().to_string()));
        }
    };
    add("Company", &c.company);
    add("Job title", &c.title);
    add("Department", &c.department);
    for e in &c.emails {
        add(if e.label.trim().is_empty() { "E-mail" } else { &e.label }, &e.value);
    }
    for ph in &c.phones {
        add(if ph.label.trim().is_empty() { "Phone" } else { &ph.label }, &ph.value);
    }
    for a in &c.addresses {
        add(if a.label.trim().is_empty() { "Address" } else { &a.label }, &a.one_line());
    }
    for w in &c.websites {
        add("Web page", w);
    }
    if let Some(b) = c.birthday {
        add("Birthday", &b.long());
    }
    add("Categories", &c.categories.join(", "));
    let notes: String = c.notes.chars().take(400).collect();
    add("Notes", &notes);
    let title = c.file_as();
    Card { title: if title.trim().is_empty() { c.name() } else { title }, fields }
}

fn tasks(s: &Session, paper: Paper) -> Document {
    let today = s.today();
    let columns = [("Subject", 4.0), ("Status", 1.3), ("Due Date", 1.4), ("Start Date", 1.4), ("Importance", 1.1), ("Categories", 1.6)]
        .into_iter()
        .map(|(title, weight)| Column { title: title.into(), weight })
        .collect::<Vec<_>>();
    let rows: Vec<Vec<String>> = super::tasks::sorted(s, TaskFilter::All)
        .into_iter()
        .filter_map(|id| s.mailbox.task(id))
        .map(|t| {
            let status = if t.is_complete() {
                "Completed"
            } else if t.is_overdue(today) {
                "Overdue"
            } else {
                "Not Started"
            };
            vec![
                t.title.clone(),
                status.to_string(),
                t.due.map(|d| d.medium()).unwrap_or_else(|| "None".into()),
                t.start.map(|d| d.medium()).unwrap_or_else(|| "None".into()),
                match t.importance {
                    Importance::High => "High".into(),
                    Importance::Low => "Low".into(),
                    Importance::Normal => "Normal".into(),
                },
                t.categories.join(", "),
            ]
        })
        .collect();
    mailcraft_print::list::table(&owner(s, None), "Tasks", &columns, &rows, paper)
}

/// The name a print's PDF is offered under.
pub fn suggested_name(s: &Session, v: &Value) -> String {
    match what(s, v) {
        Ok(What::Message) => {
            let subject = p::id(v, "id").or(s.sel.focus()).and_then(|id| s.mailbox.message(id)).map(|m| m.subject.clone()).unwrap_or_default();
            file_name(if subject.trim().is_empty() { "message" } else { &subject }, "pdf")
        }
        Ok(What::Calendar) => "Calendar.pdf".into(),
        Ok(What::Contacts) => "Contacts.pdf".into(),
        Ok(What::Tasks) | Err(_) => "Tasks.pdf".into(),
    }
}

fn print(s: &mut Session, v: &Value) -> CmdResult {
    let doc = document(s, v)?;
    let what = what(s, v)?;
    let style = match what {
        What::Message => "memo",
        What::Calendar => calendar_style(s, v)?.code(),
        What::Contacts => "card",
        What::Tasks => "table",
    };
    let pages = doc.pages.len();
    let bytes = mailcraft_print::pdf::write(&doc);
    let mut r = deliver(v, &suggested_name(s, v), bytes)?;
    if let Some(o) = r.as_object_mut() {
        o.insert("what".into(), json!(what.code()));
        o.insert("style".into(), json!(style));
        o.insert("pages".into(), json!(pages));
    }
    Ok(r)
}

#[cfg(test)]
#[path = "print_tests.rs"]
mod tests;
