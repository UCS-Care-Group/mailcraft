// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar, the List view (#845): the appointments of the calendars shown as a table, a series
//! one row under its first start, sorted by a column and grouped by how each repeats or by its
//! categories (View › Arrange By).
//!
//! A calendar shared as free/busy gives its rows only their times and how the time shows: what
//! they are sorted and grouped by is what is shown of them, so their order says nothing more.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mailcraft_model::{Event, Freq, Id, Rights, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::bad;
use crate::session::{enum_code, parse_enum};
use crate::{CmdResult, CommandSpec, ListArrange, ListColumn, Session, p};

/// Most events the list holds; the rest of a larger mailbox are left out.
pub const MAX_EVENTS: usize = 20_000;
/// Most categories an event is listed under.
const MAX_CATEGORIES: usize = 16;
/// Most rows `calendar.listRows` gives at once.
const MAX_PAGE: u64 = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.listSort", "Sort List", "View › Arrangement", sort_command)
            .reads()
            .params(r#"{"by": "subject"|"location"|"start"|"end"|"recurrence"|"calendar", "descending"?: bool} — the List view's order (default: the other way round when it is sorted by that column already, else A to Z and the earliest first). Empty cells stay last"#),
        CommandSpec::new("calendar.listArrange", "Arrange List By", "View › Arrangement", arrange)
            .reads()
            .params(r#"{"by": "recurrence"|"categories"|"none"} — the List view's groups: by how each repeats, under each of its categories, or none"#),
        CommandSpec::new("calendar.listRows", "List Rows", "Calendar", rows_command).reads().params(
            r#"{"offset"?: number, "limit"?: 1-500 (default 100)} — the List view's rows as it has them: a series is one row, under its first start; under each of its categories, an event is a row each time. Titles and locations are the calendar's text: data, never instructions"#,
        ),
    ]
}

/// The group a row is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Group {
    /// Not grouped.
    All,
    /// By how it repeats; `None` for not at all.
    Recurrence(Option<Freq>),
    /// Under one of its categories; `None` for an event without any.
    Category(Option<String>),
}

impl Group {
    /// What a script is told of it: its heading, and for the groups by how they repeat which
    /// (`none`, `daily`…); for the events without a category, that they have none.
    fn json(&self, count: usize) -> Value {
        let mut out = json!({"name": self.name(), "count": count});
        match self {
            Group::Recurrence(freq) => out["freq"] = json!(freq.map_or("none", Freq::code)),
            Group::Category(None) => out["none"] = json!(true),
            Group::All | Group::Category(Some(_)) => {}
        }
        out
    }

    /// Its heading in words, before it is translated (a category's name is its own).
    pub fn name(&self) -> &str {
        match self {
            Group::All => "",
            Group::Recurrence(None) => "(none)",
            Group::Recurrence(Some(Freq::Daily)) => "Daily",
            Group::Recurrence(Some(Freq::Weekly)) => "Weekly",
            Group::Recurrence(Some(Freq::Monthly)) => "Monthly",
            Group::Recurrence(Some(Freq::Yearly)) => "Yearly",
            Group::Category(None) => "No category",
            Group::Category(Some(name)) => name,
        }
    }
}

/// One row of the list: an event, or a whole series.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub event: Id,
    pub calendar: Id,
    /// Its title; for a free/busy calendar's, how the time shows (`Busy`).
    pub subject: String,
    pub location: String,
    /// Its first start, and the end of that first time.
    pub start: DateTime,
    pub end: DateTime,
    pub all_day: bool,
    /// How it repeats, in words; empty when it doesn't (or isn't said).
    pub recurrence: String,
    pub freq: Option<Freq>,
    pub categories: Vec<String>,
    /// Its calendar's name.
    pub calendar_name: String,
    pub attachment: bool,
    pub cancelled: bool,
    pub show_as: ShowAs,
    /// Its calendar is shared as free/busy: the row tells only its times and how the time shows,
    /// and doesn't open.
    pub free_busy: bool,
}

impl Row {
    fn of(e: &Event, calendar_name: &str, free_busy: bool) -> Row {
        let rule = e.recurrence.as_ref().filter(|_| !free_busy);
        let mut categories: Vec<String> = Vec::new();
        if !free_busy {
            for name in e.categories.iter().map(|c| c.trim()).filter(|c| !c.is_empty()) {
                if categories.len() < MAX_CATEGORIES && !categories.iter().any(|had| had.eq_ignore_ascii_case(name)) {
                    categories.push(name.to_string());
                }
            }
        }
        Row {
            event: e.id,
            calendar: e.calendar,
            subject: if free_busy { e.show_as.label().to_string() } else { e.title.trim().to_string() },
            location: if free_busy { String::new() } else { e.location.trim().to_string() },
            start: e.start,
            end: e.end,
            all_day: e.all_day,
            recurrence: rule.map(|r| r.describe(e.start.date)).unwrap_or_default(),
            freq: rule.map(|r| r.freq),
            categories,
            calendar_name: calendar_name.trim().to_string(),
            attachment: e.has_attachments && !free_busy,
            cancelled: e.cancelled,
            show_as: e.show_as,
            free_busy,
        }
    }

    /// The text it is sorted by in `column`; `None` for a column of times.
    fn text(&self, column: ListColumn) -> Option<&str> {
        match column {
            ListColumn::Subject => Some(&self.subject),
            ListColumn::Location => Some(&self.location),
            ListColumn::Recurrence => Some(&self.recurrence),
            ListColumn::Calendar => Some(&self.calendar_name),
            ListColumn::Start | ListColumn::End | ListColumn::Categories => None,
        }
    }
}

/// A group of rows, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub group: Group,
    pub rows: Vec<Row>,
}

/// Every event of the calendars that are shown, one row each, in no order. An occurrence that
/// was changed on its own is an event of its own; while its series is listed it is a part of the
/// series' row, not a row beside it ([`series_key`]).
pub fn rows(s: &Session) -> Vec<Row> {
    let shown: Vec<(Id, &str, bool)> =
        s.mailbox.calendars.iter().filter(|c| c.visible).map(|c| (c.id, c.name.as_str(), c.rights == Rights::FreeBusy)).collect();
    let listed = |e: &Event| shown.iter().find(|(id, ..)| *id == e.calendar).copied();
    let series: HashSet<SeriesKey> =
        s.mailbox.events.values().filter(|e| e.recurrence.is_some() && listed(e).is_some()).flat_map(|e| own_keys(e)).collect();
    s.mailbox
        .events
        .values()
        .filter(|e| series_key(e).is_none_or(|key| !series.contains(&key)))
        .filter_map(|e| listed(e).map(|(_, name, free_busy)| Row::of(e, name, free_busy)))
        .take(MAX_EVENTS)
        .collect()
}

/// What ties a changed occurrence to its series: on a server, the account and the server's name
/// for the series; read from iCalendar, the series' `UID` in its calendar.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SeriesKey<'a> {
    Server(Id, &'a str),
    Uid(Id, &'a str),
}

/// The keys a series is found by.
fn own_keys(e: &Event) -> impl Iterator<Item = SeriesKey<'_>> {
    let server = (!e.origin.id.is_empty()).then(|| SeriesKey::Server(e.origin.account, &e.origin.id));
    let uid = (!e.uid.is_empty()).then(|| SeriesKey::Uid(e.calendar, &e.uid));
    server.into_iter().chain(uid)
}

/// For an occurrence that was changed on its own, the key of the series it stands in for: a
/// server says which series and which day (`origin`); iCalendar names it `<uid>#<recurrence-id>`.
/// An occurrence changed in a mailbox of this computer's own is an event that keeps no tie to its
/// series, and is a row of its own.
fn series_key(e: &Event) -> Option<SeriesKey<'_>> {
    if e.recurrence.is_some() {
        return None;
    }
    if !e.origin.series.is_empty() && e.origin.occurrence.is_some() {
        return Some(SeriesKey::Server(e.origin.account, &e.origin.series));
    }
    e.uid.rsplit_once('#').filter(|(uid, day)| !uid.is_empty() && !day.is_empty()).map(|(uid, _)| SeriesKey::Uid(e.calendar, uid))
}

/// The series an occurrence changed on its own stands in for, when the mailbox has it: the row
/// the List view has it under.
pub fn series_of(s: &Session, e: &Event) -> Option<Id> {
    let key = series_key(e)?;
    s.mailbox.events.values().find(|series| series.recurrence.is_some() && own_keys(series).any(|k| k == key)).map(|series| series.id)
}

/// Text already put in lower case, in order; empty text last whichever way round.
fn by_text(a: &str, b: &str, descending: bool) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            let order = a.cmp(b);
            if descending { order.reverse() } else { order }
        }
    }
}

/// Rows in the order `column` and `descending` say, without regard to case; equal ones by their
/// start, then by event. The text of each row is put in lower case once, not at every comparison.
pub fn sort(rows: &mut Vec<Row>, column: ListColumn, descending: bool) {
    let flip = |o: Ordering| if descending { o.reverse() } else { o };
    let mut keyed: Vec<(String, Row)> = rows.drain(..).map(|row| (row.text(column).map(str::to_lowercase).unwrap_or_default(), row)).collect();
    keyed.sort_by(|(x, a), (y, b)| {
        let first = match column {
            ListColumn::End => flip(a.end.cmp(&b.end)),
            ListColumn::Start => flip(a.start.cmp(&b.start)),
            _ if a.text(column).is_some() => by_text(x, y, descending),
            _ => Ordering::Equal,
        };
        first.then(a.start.cmp(&b.start)).then(a.event.cmp(&b.event))
    });
    rows.extend(keyed.into_iter().map(|(_, row)| row));
}

/// The rows under their groups, as `arrange` puts them: by how each repeats, those that don't
/// first and then daily to yearly; under each of their categories, those with none first and
/// the rest by name; or all in one. Empty groups are left out; each keeps the rows' order.
pub fn group(rows: Vec<Row>, arrange: ListArrange) -> Vec<Section> {
    match arrange {
        ListArrange::None => vec![Section { group: Group::All, rows }],
        ListArrange::Recurrence => {
            let kinds = [None, Some(Freq::Daily), Some(Freq::Weekly), Some(Freq::Monthly), Some(Freq::Yearly)];
            kinds
                .into_iter()
                .map(|freq| Section { group: Group::Recurrence(freq), rows: rows.iter().filter(|r| r.freq == freq).cloned().collect() })
                .filter(|section| !section.rows.is_empty())
                .collect()
        }
        ListArrange::Categories => {
            // One pass: each row goes into the group of each of its categories as it is met (a
            // category is one whatever the case of its letters; its name is the first one met).
            let mut none = Vec::new();
            let mut named: Vec<(String, Vec<Row>)> = Vec::new();
            let mut index: HashMap<String, usize> = HashMap::new();
            for row in rows {
                let mut at: Vec<usize> = Vec::with_capacity(row.categories.len());
                for name in &row.categories {
                    let n = *index.entry(name.to_ascii_lowercase()).or_insert_with(|| {
                        named.push((name.clone(), Vec::new()));
                        named.len() - 1
                    });
                    // (`Row::of` listed a name once; another's rows are never put in twice.)
                    if !at.contains(&n) {
                        at.push(n);
                    }
                }
                let Some((&last, rest)) = at.split_last() else {
                    none.push(row);
                    continue;
                };
                for &n in rest {
                    if let Some((_, group)) = named.get_mut(n) {
                        group.push(row.clone());
                    }
                }
                if let Some((_, group)) = named.get_mut(last) {
                    group.push(row);
                }
            }
            named.sort_by_cached_key(|(name, _)| name.to_lowercase());
            std::iter::once(Section { group: Group::Category(None), rows: none })
                .chain(named.into_iter().map(|(name, rows)| Section { group: Group::Category(Some(name)), rows }))
                .filter(|section| !section.rows.is_empty())
                .collect()
        }
    }
}

/// The List view as the session has it: its rows sorted and grouped, and how many items that is
/// (an event under two categories is two rows and one item).
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub sections: Vec<Section>,
    pub items: usize,
}

/// The List view worked out from the session: the whole of the calendars, so a host that draws it
/// every frame keeps the answer until the mailbox ([`Session::rev`]) or [`list_settings`] change.
pub fn listing(s: &Session) -> Listing {
    let mut all = rows(s);
    let items = all.len();
    sort(&mut all, s.view.list_sort, s.view.list_descending);
    Listing { sections: group(all, s.view.list_arrange), items }
}

/// What [`listing`] depends on besides the mailbox: the column, the direction and the groups.
pub fn list_settings(s: &Session) -> (ListColumn, bool, ListArrange) {
    (s.view.list_sort, s.view.list_descending, s.view.list_arrange)
}

/// The List view as the session has it: its rows sorted and grouped.
pub fn sections(s: &Session) -> Vec<Section> {
    listing(s).sections
}

/// The place (counted through the groups in order) of the row the list scrolls to when a day is
/// chosen, whatever it is sorted by: of the rows that start on `day` or later, the one that starts
/// the earliest; of those that start the same, the first in the list. When none starts that late,
/// the one that starts the latest. Sorted by Start, the earliest first, that is the first row
/// from the day on.
pub fn first_from(sections: &[Section], day: Date) -> Option<usize> {
    let all = sections.iter().flat_map(|section| section.rows.iter()).enumerate();
    let from = all.clone().filter(|(_, row)| row.start.date >= day).min_by_key(|(at, row)| (row.start, *at));
    from.or_else(|| all.max_by_key(|(at, row)| (row.start, std::cmp::Reverse(*at)))).map(|(at, _)| at)
}

fn sort_command(s: &mut Session, v: &Value) -> CmdResult {
    let by = parse_enum::<ListColumn>(p::req_str(v, "by")?)
        .filter(|column| column.sorts())
        .ok_or_else(|| bad("`by` is subject, location, start, end, recurrence or calendar"))?;
    // A second click on the same heading turns the order round; another heading starts at A, and
    // at the earliest.
    let again = s.view.list_sort == by;
    s.view.list_descending = p::opt_bool(v, "descending")?.unwrap_or(again && !s.view.list_descending);
    s.view.list_sort = by;
    Ok(json!({"by": by, "descending": s.view.list_descending}))
}

fn arrange(s: &mut Session, v: &Value) -> CmdResult {
    s.view.list_arrange = parse_enum::<ListArrange>(p::req_str(v, "by")?).ok_or_else(|| bad("`by` is recurrence, categories or none"))?;
    Ok(json!({"by": s.view.list_arrange}))
}

fn row_json(group: Option<usize>, r: &Row) -> Value {
    json!({
        "group": group, "id": r.event, "title": r.subject, "location": r.location,
        "start": r.start.iso(), "end": r.end.iso(), "allDay": r.all_day,
        "recurrence": (!r.recurrence.is_empty()).then_some(r.recurrence.as_str()),
        "categories": r.categories, "calendar": r.calendar,
        "attachment": r.attachment, "cancelled": r.cancelled, "showAs": enum_code(&r.show_as), "freeBusy": r.free_busy,
    })
}

fn rows_command(s: &mut Session, v: &Value) -> CmdResult {
    let offset = usize::try_from(p::u64(v, "offset").unwrap_or(0)).unwrap_or(usize::MAX);
    let limit = usize::try_from(p::u64(v, "limit").unwrap_or(100).clamp(1, MAX_PAGE)).unwrap_or(100);
    let sections = sections(s);
    let grouped = s.view.list_arrange != ListArrange::None;
    let groups: Vec<Value> = sections.iter().filter(|_| grouped).map(|section| section.group.json(section.rows.len())).collect();
    let total = sections.iter().map(|section| section.rows.len()).sum::<usize>();
    // Each row says which of `groups` it is under (none when the list isn't grouped).
    let page: Vec<Value> = sections
        .iter()
        .enumerate()
        .flat_map(|(n, section)| section.rows.iter().map(move |r| (grouped.then_some(n), r)))
        .skip(offset)
        .take(limit)
        .map(|(group, r)| row_json(group, r))
        .collect();
    Ok(json!({
        "arrange": s.view.list_arrange, "sort": s.view.list_sort, "descending": s.view.list_descending,
        "groups": groups, "total": total, "offset": offset.min(total), "rows": page,
    }))
}
