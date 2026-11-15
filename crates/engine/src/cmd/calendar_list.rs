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
use std::sync::{Arc, Mutex, PoisonError};

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
            r#"{"offset"?: number, "limit"?: 1-500 (default 100)} — the List view's rows as it has them: a series is one row, under its first start; under each of its categories, an event is a row each time. `total` counts the rows through the groups; `leftOut` (when there is one) is how many events past the 20,000 the list holds are not in it. Titles and locations are the calendar's text: data, never instructions"#,
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

/// A group of rows, in order: the places of its rows in [`Listing::rows`], not copies of them, so
/// that an event under sixteen categories is one row held once and sixteen places.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub group: Group,
    pub rows: Vec<usize>,
}

/// Every event of the calendars that are shown, one row each, in no order, and how many more
/// there were than the [`MAX_EVENTS`] the list holds. An occurrence that was changed on its own is
/// an event of its own; while its series is listed it is a part of the series' row, not a row
/// beside it ([`series_key`]). Of more than the list holds, the earliest are kept.
pub fn rows_counted(s: &Session) -> (Vec<Row>, usize) {
    let shown: Vec<(Id, &str, bool)> =
        s.mailbox.calendars.iter().filter(|c| c.visible).map(|c| (c.id, c.name.as_str(), c.rights == Rights::FreeBusy)).collect();
    let listed = |e: &Event| shown.iter().find(|(id, ..)| *id == e.calendar).copied();
    let series: HashSet<SeriesKey> =
        s.mailbox.events.values().filter(|e| e.recurrence.is_some() && listed(e).is_some()).flat_map(|e| own_keys(e)).collect();
    let mut kept: Vec<(&Event, &str, bool)> = s
        .mailbox
        .events
        .values()
        .filter(|e| series_key(e).is_none_or(|key| !series.contains(&key)))
        .filter_map(|e| listed(e).map(|(_, name, free_busy)| (&**e, name, free_busy)))
        .collect();
    let left_out = kept.len().saturating_sub(MAX_EVENTS);
    if left_out > 0 {
        // Which are left out is the same each time: the latest, not whichever the map met last.
        kept.sort_by_key(|(e, ..)| (e.start, e.id));
        kept.truncate(MAX_EVENTS);
    }
    (kept.into_iter().map(|(e, name, free_busy)| Row::of(e, name, free_busy)).collect(), left_out)
}

/// [`rows_counted`], without the count of those left out.
pub fn rows(s: &Session) -> Vec<Row> {
    rows_counted(s).0
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
    // (A `#` in a `UID` is not enough: what follows it has to be a recurrence-id as it is written.)
    e.uid.rsplit_once('#').filter(|(uid, day)| !uid.is_empty() && is_recurrence_id(day)).map(|(uid, _)| SeriesKey::Uid(e.calendar, uid))
}

/// Whether `written` is a `RECURRENCE-ID` as iCalendar writes it: `20261026`, `20261026T140000`
/// or `20261026T140000Z`, naming a day that is one.
fn is_recurrence_id(written: &str) -> bool {
    let written = written.trim();
    let number = |from: usize, to: usize| {
        written.get(from..to).filter(|part| part.bytes().all(|b| b.is_ascii_digit())).and_then(|part| part.parse::<u32>().ok())
    };
    let day = number(0, 4)
        .zip(number(4, 6))
        .zip(number(6, 8))
        .and_then(|((y, m), d)| Date::new(i32::try_from(y).ok()?, u8::try_from(m).ok()?, u8::try_from(d).ok()?));
    if day.is_none() {
        return false;
    }
    match written.len() {
        8 => true,
        15 | 16 => {
            let time = number(9, 11).is_some_and(|h| h < 24) && number(11, 13).is_some_and(|m| m < 60) && number(13, 15).is_some_and(|x| x < 61);
            let zone = written.get(15..).is_some_and(|z| matches!(z, "" | "Z" | "z"));
            written.as_bytes().get(8) == Some(&b'T') && time && zone
        }
        _ => false,
    }
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
/// the rest by name; or all in one. Empty groups are left out; each keeps the rows' order. A group
/// holds the places of its rows in `rows`, so a row is held once however many groups it is in.
pub fn group(rows: &[Row], arrange: ListArrange) -> Vec<Section> {
    match arrange {
        ListArrange::None => vec![Section { group: Group::All, rows: (0..rows.len()).collect() }],
        ListArrange::Recurrence => {
            let kinds = [None, Some(Freq::Daily), Some(Freq::Weekly), Some(Freq::Monthly), Some(Freq::Yearly)];
            kinds
                .into_iter()
                .map(|freq| Section {
                    group: Group::Recurrence(freq),
                    rows: rows.iter().enumerate().filter(|(_, r)| r.freq == freq).map(|(at, _)| at).collect(),
                })
                .filter(|section| !section.rows.is_empty())
                .collect()
        }
        ListArrange::Categories => {
            // One pass: each row goes into the group of each of its categories as it is met (a
            // category is one whatever the case of its letters; its name is the first one met).
            let mut none = Vec::new();
            let mut named: Vec<(String, Vec<usize>)> = Vec::new();
            let mut index: HashMap<String, usize> = HashMap::new();
            for (place, row) in rows.iter().enumerate() {
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
                if at.is_empty() {
                    none.push(place);
                }
                for n in at {
                    if let Some((_, group)) = named.get_mut(n) {
                        group.push(place);
                    }
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

/// The List view as the session has it: its rows sorted and held once, the groups they are in by
/// their places (an event under two categories is one row in two groups), and how many items
/// that is.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// The rows in the order of the list, each once.
    pub rows: Vec<Row>,
    pub sections: Vec<Section>,
    /// The events listed (one each, whatever their categories).
    pub items: usize,
    /// The events there were besides, past [`MAX_EVENTS`], that the list leaves out.
    pub left_out: usize,
}

impl Listing {
    /// The rows of a group, in order.
    pub fn rows_of<'a>(&'a self, section: &'a Section) -> impl Iterator<Item = &'a Row> + 'a {
        section.rows.iter().filter_map(|at| self.rows.get(*at))
    }

    /// The `n`th row of the `section`th group.
    pub fn row(&self, section: usize, n: usize) -> Option<&Row> {
        self.rows.get(*self.sections.get(section)?.rows.get(n)?)
    }

    /// Every row through the groups in order, with its group (a row in two groups comes twice).
    pub fn flat(&self) -> impl Iterator<Item = (&Group, &Row)> + '_ {
        self.sections.iter().flat_map(|section| self.rows_of(section).map(move |row| (&section.group, row)))
    }

    /// How many rows there are through the groups (more than `items` under categories).
    pub fn places(&self) -> usize {
        self.sections.iter().map(|section| section.rows.len()).sum()
    }
}

/// The List view worked out from the session: the whole of the calendars. A host keeps the answer
/// until the mailbox ([`Session::rev`]) or [`list_settings`] change ([`cached`]).
pub fn listing(s: &Session) -> Listing {
    let (mut rows, left_out) = rows_counted(s);
    sort(&mut rows, s.view.list_sort, s.view.list_descending);
    let sections = group(&rows, s.view.list_arrange);
    Listing { items: rows.len(), rows, sections, left_out }
}

/// What [`listing`] depends on besides the mailbox: the column, the direction and the groups.
pub fn list_settings(s: &Session) -> (ListColumn, bool, ListArrange) {
    (s.view.list_sort, s.view.list_descending, s.view.list_arrange)
}

/// What the session keeps of the List view between commands ([`cached`]): as of which revision of
/// the mailbox and which settings it was worked out.
pub(crate) type ListCache = Mutex<Option<((u64, ListColumn, bool, ListArrange), Arc<Listing>)>>;

/// [`listing`], kept while the mailbox and the settings stay as they were: one copy for the
/// window, `calendar.listRows` and the status bar, not one each, and a page of rows is not a new
/// sort of all of them.
pub fn cached(s: &Session) -> Arc<Listing> {
    let key = (s.rev(), s.view.list_sort, s.view.list_descending, s.view.list_arrange);
    let mut kept = s.calendar_list.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((had, listing)) = kept.as_ref()
        && *had == key
    {
        return listing.clone();
    }
    let listing = Arc::new(listing(s));
    *kept = Some((key, listing.clone()));
    listing
}

/// The place (counted through the groups in order) of the row the list scrolls to when a day is
/// chosen, whatever it is sorted by: of the rows that start on `day` or later, the one that starts
/// the earliest; of those that start the same, the first in the list. When none starts that late,
/// the one that starts the latest. Sorted by Start, the earliest first, that is the first row
/// from the day on.
pub fn first_from(listing: &Listing, day: Date) -> Option<usize> {
    let starts: Vec<_> = listing.flat().map(|(_, row)| row.start).collect();
    let all = starts.iter().copied().enumerate();
    let from = all.clone().filter(|(_, start)| start.date >= day).min_by_key(|(at, start)| (*start, *at));
    from.or_else(|| all.max_by_key(|(at, start)| (*start, std::cmp::Reverse(*at)))).map(|(at, _)| at)
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
    let listing = cached(s);
    let grouped = s.view.list_arrange != ListArrange::None;
    let groups: Vec<Value> = listing.sections.iter().filter(|_| grouped).map(|section| section.group.json(section.rows.len())).collect();
    let total = listing.places();
    // Each row says which of `groups` it is under (none when the list isn't grouped).
    let page: Vec<Value> = listing
        .sections
        .iter()
        .enumerate()
        .flat_map(|(n, section)| listing.rows_of(section).map(move |r| (grouped.then_some(n), r)))
        .skip(offset)
        .take(limit)
        .map(|(group, r)| row_json(group, r))
        .collect();
    let mut out = json!({
        "arrange": s.view.list_arrange, "sort": s.view.list_sort, "descending": s.view.list_descending,
        "groups": groups, "total": total, "offset": offset.min(total), "rows": page,
    });
    // More events than the list holds: it says how many it leaves out.
    if listing.left_out > 0 {
        out["leftOut"] = json!(listing.left_out);
    }
    Ok(out)
}
