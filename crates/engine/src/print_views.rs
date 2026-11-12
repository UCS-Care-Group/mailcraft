// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar, contacts and tasks as documents to print (#32 part 2): each built as display
//! blocks (headings, paragraphs, tables), so they are laid out on pages as a message's body is.
//! What is printed is what the module shows: the calendar's days in view, the contacts the
//! People list holds, the tasks the Tasks list holds, in their order.

use mailcraft_html::{Block, BlockKind, Cell, Insets, Line, Rgb, Row, Sides, Span, Table, VAlign};
use mailcraft_i18n::date::{self, Day};
use mailcraft_model::{Contact, Id, Importance, Task};
use mailcraft_print::Doc;
use mailcraft_time::{Date, Weekday};

use crate::print::Labels;
use crate::{CmdError, Session, cmd::fail};

/// The most days of the calendar one print takes.
pub const MAX_DAYS: i64 = 400;
/// The most contacts or tasks one print takes.
pub const MAX_ITEMS: usize = 5000;
/// The most appointments written in one day of a month.
const MAX_IN_A_DAY: usize = 8;

const MUTED: Rgb = Rgb(0x60, 0x60, 0x60);
const OUTSIDE: Rgb = Rgb(0xf4, 0xf4, 0xf4);
const HEADER: Rgb = Rgb(0xe9, 0xed, 0xf3);

/// How the calendar is printed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalendarStyle {
    /// A grid of weeks for each month the days touch.
    Month,
    /// Each day with what happens on it, in order.
    #[default]
    Agenda,
}

fn day_of(d: Date) -> Day {
    Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() }
}

fn para(spans: Vec<Span>) -> Block {
    Block::new(BlockKind::Paragraph, spans)
}

fn text(t: impl Into<String>) -> Span {
    Span::plain(t)
}

fn muted(t: impl Into<String>) -> Span {
    Span { color: Some(MUTED), ..Span::plain(t) }
}

fn bold(t: impl Into<String>) -> Span {
    Span { bold: true, ..Span::plain(t) }
}

fn small(mut span: Span) -> Span {
    span.size = Some(12);
    span
}

fn cell(blocks: Vec<Block>) -> Cell {
    Cell {
        blocks,
        header: false,
        colspan: 1,
        rowspan: 1,
        background: None,
        width: None,
        border: Sides::default(),
        lines: [Line::default(); 4],
        padding: Some(Insets { top: 4, right: 5, bottom: 4, left: 5 }),
        radius: 0,
        valign: Some(VAlign::Top),
    }
}

fn header_cell(label: &str) -> Cell {
    Cell { header: true, background: Some(HEADER), ..cell(vec![para(vec![text(label)])]) }
}

fn table(rows: Vec<Row>, columns: u16, border: bool) -> Block {
    let t = Table { rows, columns, layout: !border, border, outline: border, width: Some(mailcraft_html::Length::Percent(100)), ..Table::default() };
    Block::new(BlockKind::Table(t), Vec::new())
}

fn spaced(mut b: Block, space: u8) -> Block {
    b.space = space;
    b
}

impl Session {
    /// The calendar's days `from`–`to` as a document.
    pub fn print_calendar(&self, from: Date, to: Date, style: CalendarStyle, labels: &Labels) -> Result<Doc, CmdError> {
        if to < from || from.days_until(to) >= MAX_DAYS {
            return Err(fail(format!("print at most {MAX_DAYS} days of the calendar at a time")));
        }
        let lang = self.dated();
        let range = if from == to {
            date::long(&lang, day_of(from))
        } else {
            format!("{} – {}", date::long_without_weekday(&lang, day_of(from)), date::long_without_weekday(&lang, day_of(to)))
        };
        let occurrences = crate::cmd::calendar::occurrences(self, from, to);
        let mut blocks = Vec::new();
        match style {
            CalendarStyle::Agenda => {
                let mut day = from;
                let mut first = true;
                while day <= to {
                    let today: Vec<_> = occurrences.iter().filter(|o| o.day == day || (o.start.date <= day && o.end.date > day)).collect();
                    let heading = Block::new(BlockKind::Heading(3), vec![text(date::long(&lang, day_of(day)))]);
                    blocks.push(spaced(heading, if first { 0 } else { 2 }));
                    first = false;
                    if today.is_empty() {
                        blocks.push(para(vec![muted(labels.nothing.clone())]));
                    }
                    let mut rows = Vec::new();
                    for o in today {
                        let Some(e) = self.mailbox.event(o.event) else { continue };
                        let when =
                            if e.all_day { labels.all_day.clone() } else { format!("{} – {}", self.time_said(o.start), self.time_said(o.end)) };
                        let mut what = vec![bold(e.title.clone())];
                        if !e.location.trim().is_empty() {
                            what.push(muted(format!("  ·  {}", e.location.trim())));
                        }
                        rows.push(Row {
                            cells: vec![
                                Cell { width: Some(mailcraft_html::Length::Percent(28)), ..cell(vec![para(vec![muted(when)])]) },
                                cell(vec![para(what)]),
                            ],
                        });
                    }
                    if !rows.is_empty() {
                        blocks.push(table(rows, 2, false));
                    }
                    day = day.add_days(1);
                }
            }
            CalendarStyle::Month => {
                let week_start = self.prefs.week_start;
                let mut month = from.start_of_month();
                let mut first = true;
                while month <= to {
                    let heading = Block::new(BlockKind::Heading(2), vec![text(date::month_year(&lang, month.year, month.month))]);
                    blocks.push(spaced(heading, if first { 0 } else { 2 }));
                    first = false;
                    let names: Vec<Cell> = (0..7)
                        .map(|i| Cell {
                            width: Some(mailcraft_html::Length::Percent(14)),
                            ..header_cell(date::weekday_short(&lang, Weekday::from_index(i64::from(week_start.index()) + i).index()))
                        })
                        .collect();
                    let mut rows = vec![Row { cells: names }];
                    let mut day = month.start_of_week(week_start);
                    let end = month.end_of_month();
                    while day <= end {
                        let mut cells = Vec::new();
                        for _ in 0..7 {
                            let mut lines = vec![para(vec![bold(day.day.to_string())])];
                            let today: Vec<_> = occurrences.iter().filter(|o| o.day == day).collect();
                            for o in today.iter().take(MAX_IN_A_DAY) {
                                let Some(e) = self.mailbox.event(o.event) else { continue };
                                let line = if e.all_day { e.title.clone() } else { format!("{} {}", self.time_said(o.start), e.title) };
                                lines.push(para(vec![small(text(line))]));
                            }
                            if today.len() > MAX_IN_A_DAY {
                                lines.push(para(vec![small(muted(format!("+{}", today.len() - MAX_IN_A_DAY)))]));
                            }
                            let mut c = cell(lines);
                            if day.month != month.month || day < from || day > to {
                                c.background = Some(OUTSIDE);
                            }
                            cells.push(c);
                            day = day.add_days(1);
                        }
                        rows.push(Row { cells });
                    }
                    blocks.push(table(rows, 7, true));
                    month = month.add_months(1);
                }
            }
        }
        Ok(Doc { title: labels.calendar.clone(), fields: vec![(String::new(), range)], blocks, pictures: Vec::new() })
    }

    /// The contacts `ids`, or those the People list shows, as a document.
    pub fn print_contacts(&self, ids: &[Id], labels: &Labels) -> Result<Doc, CmdError> {
        let ids: Vec<Id> = if ids.is_empty() { crate::cmd::people::sorted(self, &self.view.people_search) } else { ids.to_vec() };
        if ids.is_empty() {
            return Err(fail("there are no contacts to print"));
        }
        if ids.len() > MAX_ITEMS {
            return Err(fail(format!("print at most {MAX_ITEMS} contacts at a time")));
        }
        let mut blocks = Vec::new();
        for (n, id) in ids.iter().enumerate() {
            let c = self.mailbox.contact(*id).ok_or_else(|| failed!("no contact with id {id}", id = *id))?;
            blocks.extend(contact_blocks(self, c, labels, n == 0));
        }
        Ok(Doc { title: labels.contacts.clone(), fields: Vec::new(), blocks, pictures: Vec::new() })
    }

    /// The tasks `ids`, or those the Tasks list shows, as a document.
    pub fn print_tasks(&self, ids: &[Id], labels: &Labels) -> Result<Doc, CmdError> {
        let ids: Vec<Id> = if ids.is_empty() { crate::cmd::tasks::sorted(self, self.view.task_filter) } else { ids.to_vec() };
        if ids.is_empty() {
            return Err(fail("there are no tasks to print"));
        }
        if ids.len() > MAX_ITEMS {
            return Err(fail(format!("print at most {MAX_ITEMS} tasks at a time")));
        }
        let lang = self.dated();
        let head = [&labels.subject, &labels.due, &labels.status, &labels.importance, &labels.categories];
        let shares = [40u8, 14, 18, 12, 16];
        let mut rows = vec![Row {
            cells: head.iter().zip(shares).map(|(l, p)| Cell { width: Some(mailcraft_html::Length::Percent(p)), ..header_cell(l) }).collect(),
        }];
        for id in &ids {
            let t: &Task = self.mailbox.task(*id).ok_or_else(|| failed!("no task with id {id}", id = *id))?;
            let mut subject = vec![para(vec![text(t.title.clone())])];
            let notes = t.notes.trim();
            if !notes.is_empty() {
                let short: String = notes.chars().take(300).collect();
                subject.push(para(vec![small(muted(short))]));
            }
            let due = t.due.map(|d| date::medium(&lang, day_of(d))).unwrap_or_default();
            let status = match t.completed {
                Some(at) => format!("{} {}", labels.completed, date::medium(&lang, day_of(self.local(at).date))),
                None => labels.not_completed.clone(),
            };
            let importance = match t.importance {
                Importance::High => labels.high.clone(),
                Importance::Low => labels.low.clone(),
                Importance::Normal => labels.normal.clone(),
            };
            rows.push(Row {
                cells: vec![
                    cell(subject),
                    cell(vec![para(vec![text(due)])]),
                    cell(vec![para(vec![text(status)])]),
                    cell(vec![para(vec![text(importance)])]),
                    cell(vec![para(vec![text(t.categories.join(", "))])]),
                ],
            });
        }
        Ok(Doc { title: labels.tasks.clone(), fields: Vec::new(), blocks: vec![table(rows, 5, true)], pictures: Vec::new() })
    }
}

/// One contact: its name as a heading, then what is known of it, a line each.
fn contact_blocks(s: &Session, c: &Contact, labels: &Labels, first: bool) -> Vec<Block> {
    let mut out = vec![spaced(Block::new(BlockKind::Heading(3), vec![text(c.name())]), if first { 0 } else { 2 })];
    let work: Vec<&str> =
        [c.title.as_str(), c.department.as_str(), c.company.as_str()].into_iter().map(str::trim).filter(|t| !t.is_empty()).collect();
    if !work.is_empty() {
        out.push(para(vec![muted(work.join(", "))]));
    }
    let line = |label: &str, value: &str| -> Block {
        let label = if label.trim().is_empty() { String::new() } else { format!("{}: ", label.trim()) };
        para(vec![muted(label), text(value.trim())])
    };
    for e in c.emails.iter().filter(|e| !e.value.trim().is_empty()) {
        out.push(line(if e.label.is_empty() { &labels.email } else { &e.label }, &e.value));
    }
    for p in c.phones.iter().filter(|p| !p.value.trim().is_empty()) {
        out.push(line(if p.label.is_empty() { &labels.phone } else { &p.label }, &p.value));
    }
    for a in &c.addresses {
        let place = [a.postal_code.trim(), a.city.trim()].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
        let parts: Vec<&str> = [a.street.trim(), place.as_str(), a.region.trim(), a.country.trim()].into_iter().filter(|t| !t.is_empty()).collect();
        if !parts.is_empty() {
            out.push(line(if a.label.is_empty() { &labels.address } else { &a.label }, &parts.join(", ")));
        }
    }
    for w in c.websites.iter().filter(|w| !w.trim().is_empty()) {
        out.push(para(vec![text(w.trim())]));
    }
    if let Some(b) = c.birthday {
        out.push(line(&labels.birthday, &date::long_without_weekday(&s.dated(), day_of(b))));
    }
    let notes = c.notes.trim();
    if !notes.is_empty() {
        out.push(line(&labels.notes, &notes.chars().take(2000).collect::<String>()));
    }
    out
}
