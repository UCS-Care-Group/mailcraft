// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Advanced Search (#329): the fields of the panel under the search box, read out of the box's
//! query and written back into it as keywords the search reads.
//!
//! The panel reads what it can show: the first keyword for each of its fields (not one with a `-`
//! in front, but for attachments, where `-has:attachment` is No), and the days received when they
//! are whole dates. Anything else stays in the box as it was typed (words, other keywords, a
//! second `from:`, a date without a year, a day in words, `before:`), and is left alone when
//! Search writes the fields back: only a field that was changed is written, where its keyword
//! stood ([`Reading::write`]).

use std::ops::{Bound, Range};

use mailcraft_time::Date;

use super::{Kind, Query, is_keyword, keyword_value, opening, scan, starts_with_keyword_at, tokens};
use crate::Importance;

/// What the Advanced Search panel asks for. An empty field asks for nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Advanced {
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub category: String,
    /// Words in an attachment's file name.
    pub attachment: String,
    /// With attachments (`Some(true)`), without (`Some(false)`), either (`None`).
    pub attachments: Option<bool>,
    /// The first and the last day a message was received on, both in it.
    pub received_from: Option<Date>,
    pub received_to: Option<Date>,
    /// Read (`Some(true)`), unread (`Some(false)`), either (`None`).
    pub read: Option<bool>,
    pub importance: Option<Importance>,
}

/// A field of the panel, in the order the panel shows them (Outlook's, with the three Add filters
/// puts in it where Outlook puts them), which is the order new keywords are written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    From,
    To,
    Cc,
    Subject,
    Received,
    Read,
    Attachments,
    Attachment,
    Category,
    Importance,
    Body,
}

impl Field {
    pub const ALL: [Field; 11] = [
        Field::From,
        Field::To,
        Field::Cc,
        Field::Subject,
        Field::Received,
        Field::Read,
        Field::Attachments,
        Field::Attachment,
        Field::Category,
        Field::Importance,
        Field::Body,
    ];

    /// The words of the field `key` (a keyword in small letters) fills.
    fn of_words(key: &str) -> Option<Field> {
        Some(match key {
            "from" => Field::From,
            "to" => Field::To,
            "cc" => Field::Cc,
            "subject" => Field::Subject,
            "body" => Field::Body,
            "category" | "categories" => Field::Category,
            "attachment" | "attachments" => Field::Attachment,
            _ => return None,
        })
    }
}

impl Advanced {
    /// Whether `field` says the same in both.
    fn same(&self, other: &Advanced, field: Field) -> bool {
        match field {
            Field::Received => (self.received_from, self.received_to) == (other.received_from, other.received_to),
            Field::Read => self.read == other.read,
            Field::Attachments => self.attachments == other.attachments,
            Field::Importance => self.importance == other.importance,
            _ => self.text(field).map(|s| s.trim()) == other.text(field).map(|s| s.trim()),
        }
    }

    /// The text of a field of words.
    fn text(&self, field: Field) -> Option<&str> {
        Some(match field {
            Field::From => &self.from,
            Field::To => &self.to,
            Field::Cc => &self.cc,
            Field::Subject => &self.subject,
            Field::Body => &self.body,
            Field::Category => &self.category,
            Field::Attachment => &self.attachment,
            _ => return None,
        })
    }

    fn text_mut(&mut self, field: Field) -> Option<&mut String> {
        Some(match field {
            Field::From => &mut self.from,
            Field::To => &mut self.to,
            Field::Cc => &mut self.cc,
            Field::Subject => &mut self.subject,
            Field::Body => &mut self.body,
            Field::Category => &mut self.category,
            Field::Attachment => &mut self.attachment,
            _ => return None,
        })
    }

    /// The keyword `field` is written as; `None` when it asks for nothing.
    fn term(&self, field: Field) -> Option<String> {
        let key = match field {
            Field::From => "from",
            Field::To => "to",
            Field::Cc => "cc",
            Field::Subject => "subject",
            Field::Body => "body",
            Field::Category => "category",
            Field::Attachment => "attachment",
            Field::Attachments => return self.attachments.map(|with| if with { "has:attachment" } else { "-has:attachment" }.to_string()),
            Field::Read => return self.read.map(|read| if read { "is:read" } else { "is:unread" }.to_string()),
            Field::Importance => return self.importance.map(|i| format!("importance:{}", i.code())),
            Field::Received => {
                return match (self.received_from, self.received_to) {
                    (Some(a), Some(b)) if a == b => Some(format!("received:{}", a.iso())),
                    (Some(a), Some(b)) => Some(format!("received:{}..{}", a.iso(), b.iso())),
                    (Some(a), None) => Some(format!("received:>={}", a.iso())),
                    (None, Some(b)) => Some(format!("received:<={}", b.iso())),
                    (None, None) => None,
                };
            }
        };
        let value = self.text(field)?.trim();
        let term = format!("{key}:{}", keyword_value(value));
        // Text the search would pass over (`@@`, `💰`: no letter or digit) is written as nothing,
        // so that the box never holds a keyword that doesn't search.
        (!Query::parse(&term).is_empty()).then_some(term)
    }

    /// Every field empty.
    pub fn is_empty(&self) -> bool {
        *self == Advanced::default()
    }
}

/// A query read for the panel: its fields, and where in the query each came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    pub fields: Advanced,
    query: String,
    /// Each field read, with where its keywords stand in `query` (two for days received written
    /// as two keywords, `received:>=A received:<=B`), in order.
    taken: Vec<(Field, Vec<Range<usize>>)>,
}

/// The first and the last day of a `received:` range, as the panel's two days: `>A` is the day
/// after A, `<B` the day before B. `None` when one side is a day the calendar has no day after
/// (or before).
fn days(first: Bound<Date>, last: Bound<Date>) -> Option<(Option<Date>, Option<Date>)> {
    let side = |b: Bound<Date>, step: i64| match b {
        Bound::Included(d) => Some(Some(d)),
        Bound::Excluded(d) => {
            let next = d.add_days(step);
            (next != d).then_some(Some(next))
        }
        Bound::Unbounded => Some(None),
    };
    Some((side(first, 1)?, side(last, -1)?))
}

impl Reading {
    /// The fields `query` fills, as the search reads it (within the terms it keeps).
    pub fn of(query: &str) -> Reading {
        let mut r = Reading { query: query.to_string(), ..Reading::default() };
        for term in scan(query) {
            let Some(kind) = term.keyword.filter(|k| !k.asks_nothing()) else { continue };
            let (key, value) = term.body.split_once(':').unwrap_or_default();
            let key = key.to_ascii_lowercase();
            let negate = term.negate;
            let field = match kind {
                Kind::Words { .. } | Kind::Address { .. } | Kind::Category(_) if !negate => Field::of_words(&key),
                Kind::HasAttachment => Some(Field::Attachments),
                Kind::Read | Kind::Unread if !negate => Some(Field::Read),
                Kind::Importance(_) if !negate && key == "importance" => Some(Field::Importance),
                Kind::Received(..) if !negate => Some(Field::Received),
                _ => None,
            };
            let Some(field) = field else { continue };
            let read = r.taken.iter().position(|(f, _)| *f == field);
            match (field, kind) {
                // Days received may come as two keywords, one for each side.
                (Field::Received, Kind::Received(first, last)) => {
                    let Some((from, to)) = days(first, last) else { continue };
                    let a = &mut r.fields;
                    let fits = (from.is_none() || a.received_from.is_none()) && (to.is_none() || a.received_to.is_none());
                    match read {
                        None => r.taken.push((field, vec![term.range])),
                        Some(i) if fits => {
                            if let Some((_, ranges)) = r.taken.get_mut(i) {
                                ranges.push(term.range);
                            }
                        }
                        Some(_) => continue,
                    }
                    a.received_from = a.received_from.or(from);
                    a.received_to = a.received_to.or(to);
                }
                _ if read.is_some() => {}
                (Field::Attachments, _) => {
                    r.fields.attachments = Some(!negate);
                    r.taken.push((field, vec![term.range]));
                }
                (Field::Read, kind) => {
                    r.fields.read = Some(kind == Kind::Read);
                    r.taken.push((field, vec![term.range]));
                }
                (Field::Importance, Kind::Importance(i)) => {
                    r.fields.importance = Some(i);
                    r.taken.push((field, vec![term.range]));
                }
                (field, _) => {
                    if let Some(text) = r.fields.text_mut(field) {
                        *text = value.to_string();
                        r.taken.push((field, vec![term.range]));
                    }
                }
            }
        }
        r
    }

    /// The query read.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The query with `fields` in place of those read: a field that was changed is written where
    /// its keyword stood (and taken out when it was emptied), a field that was empty is added at
    /// the end, in the panel's order; everything else, and every field left as it was, stays as
    /// it was typed. When added keywords at the end wouldn't be read (after a quote left open, or
    /// past the terms the search keeps), they go at the start.
    pub fn write(&self, fields: &Advanced) -> String {
        let changed: Vec<Field> = Field::ALL.into_iter().filter(|f| !self.fields.same(fields, *f)).collect();
        // In place, from the end of the query to its start, so that what is still to come stands
        // where it was read.
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let mut added: Vec<String> = Vec::new();
        for field in &changed {
            let term = fields.term(*field);
            match self.taken.iter().find(|(f, _)| f == field) {
                Some((_, ranges)) => {
                    for (i, range) in ranges.iter().enumerate() {
                        let with = if i == 0 { term.clone().unwrap_or_default() } else { String::new() };
                        edits.push((range.clone(), with));
                    }
                }
                None => added.extend(term),
            }
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let mut out = self.query.clone();
        for (range, with) in edits {
            let start = if with.is_empty() { waiting(&out, &range) } else { range.start };
            replace(&mut out, start..range.end, &with);
        }
        if added.is_empty() {
            return out;
        }
        let added = added.join(" ");
        let gap = |s: &str| if s.is_empty() || s.ends_with(char::is_whitespace) { "" } else { " " };
        let at_end = format!("{out}{}{added}", gap(&out));
        let wanted = |q: &str| {
            let read = Reading::of(q).fields;
            changed.iter().all(|f| read.same(fields, *f))
        };
        if wanted(&at_end) {
            return at_end;
        }
        let rest = out.trim_start();
        format!("{added}{}{rest}", if rest.is_empty() { "" } else { " " })
    }
}

/// Where a keyword at `range` of `s` that is to be taken out starts, with the keywords before it
/// that have nothing after their colon (`subject: is:read budget`): they ask for nothing while a
/// keyword follows them, and would take the word after it as their value once it is gone.
fn waiting(s: &str, range: &Range<usize>) -> usize {
    let all = tokens(s);
    let takes_next = all.iter().find(|(_, at)| at.start >= range.end).is_some_and(|(next, at)| !starts_with_keyword_at(s, next, at));
    if !takes_next {
        return range.start;
    }
    let bare = |(token, at): &&(String, Range<usize>)| {
        !opening(s, at).1 && token.strip_prefix('-').unwrap_or(token).strip_suffix(':').is_some_and(is_keyword)
    };
    all.iter().rev().filter(|(_, at)| at.end <= range.start).take_while(bare).last().map_or(range.start, |(_, at)| at.start)
}

/// `range` of `s` replaced with `with`; when `with` is empty, the spaces after it go too (before
/// it, at the end of `s`), so that no run of spaces is left where it stood.
fn replace(s: &mut String, range: Range<usize>, with: &str) {
    if s.get(range.clone()).is_none() {
        return;
    }
    let (mut start, mut end) = (range.start, range.end);
    if with.is_empty() {
        let after = s.get(end..).unwrap_or_default();
        let spaces = after.len().saturating_sub(after.trim_start().len());
        if spaces > 0 && spaces < after.len() {
            end = end.saturating_add(spaces);
        } else {
            let before = s.get(..start).unwrap_or_default();
            start = before.trim_end().len();
            end = end.saturating_add(spaces);
        }
    }
    s.replace_range(start..end, with);
}
