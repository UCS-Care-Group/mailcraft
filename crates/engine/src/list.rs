// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list: which messages show for the folder, filter and search in view, in what
//! order, under which group headings.

use std::collections::HashMap;

use mailcraft_model::search::Query;
use mailcraft_model::{FlagStatus, FolderKind, Id, Importance, Message};
use mailcraft_time::{DateGroup, Timestamp};
use serde::Serialize;

use crate::{Filter, Scope, Session, SortBy};

/// One row of the message list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    /// The message the row shows (the newest of its conversation).
    pub id: Id,
    /// Every message the row stands for, newest first (just `id` outside conversation view).
    pub members: Vec<Id>,
    pub unread: usize,
    /// The group heading above this row, when it starts a group (`Today`, `Last Week`).
    pub group: Option<String>,
    /// Which of the message's rows this is: arranged by categories, a message filed under two
    /// categories is listed under each, as Outlook lists it, and the second row is copy 1.
    pub copy: u16,
}

/// When a message counts as having arrived: its received time, or its date when it has none.
pub fn when(m: &Message) -> Timestamp {
    if m.received != 0 { m.received } else { m.date }
}

/// The first words of a message on one line, at most `max_chars` characters (and `…` when there
/// is more): what a message list shows under the subject.
///
/// It is what the message says to someone reading it: taken from the HTML when there is HTML
/// (the opening line a sender hid for message lists included), otherwise from the text, with
/// markup, character references, invisible padding, rules, quoted text and web addresses left out
/// ([`mailcraft_html::preview`], [`mailcraft_html::text_preview`]). It is worked out from the
/// message each time and stored nowhere, so it costs a short read of the body's first words: a
/// list that draws the same rows again and again keeps what it was given.
pub fn preview(m: &Message, max_chars: usize) -> String {
    let said = m.body.html.as_deref().map(|html| mailcraft_html::preview(html, max_chars)).unwrap_or_default();
    if said.is_empty() { mailcraft_html::text_preview(&m.body.text, max_chars) } else { said }
}

fn group_label(s: &Session, m: &Message) -> Option<String> {
    if m.pinned {
        return Some("Pinned".into());
    }
    match s.view.sort {
        SortBy::Date => Some(DateGroup::of(s.local(when(m)).date, s.today(), s.prefs.week_start).label().to_string()),
        SortBy::From => Some(m.from.display().to_string()),
        SortBy::Importance => Some(
            match m.importance {
                Importance::High => "High",
                Importance::Normal => "Normal",
                Importance::Low => "Low",
            }
            .to_string(),
        ),
        // Each row of a message arranged by categories has a heading of its own (`by_category`).
        SortBy::Categories => None,
        SortBy::Subject | SortBy::Size => None,
    }
}

/// The heading of the things that have no category, in a list arranged by categories.
pub const NO_CATEGORY: &str = "No category";

/// An item's categories one by one, A to Z, each once and spelt as the mailbox's list of
/// categories spells it: the headings an item arranged by categories is listed under.
pub fn category_names<'a>(s: &'a Session, categories: &'a [String]) -> Vec<&'a str> {
    let mut names: Vec<&str> = categories
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .map(|c| s.mailbox.categories.iter().find(|k| k.name.eq_ignore_ascii_case(c)).map_or(c, |k| k.name.as_str()))
        .collect();
    names.sort_by_key(|c| c.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

pub fn rows(s: &Session) -> Vec<Row> {
    rows_with(s, &HashMap::new(), &HashMap::new())
}

/// Which messages the list looks at: those of the folder in view, or, for a search of every
/// mailbox, every message but those in Deleted Items, Junk E-mail and their subfolders.
pub(crate) fn in_view(s: &Session) -> impl Fn(&Message) -> bool + '_ {
    let mb = &s.mailbox;
    let everywhere = !Query::parse(&s.view.search).is_empty() && s.view.scope == Scope::Everywhere;
    // Deleted and junk mail stays out of a search of everything.
    let hidden: std::collections::HashSet<Id> = if everywhere {
        mb.folders.iter().filter(|f| matches!(f.kind, FolderKind::Deleted | FolderKind::Junk)).flat_map(|f| mb.subtree(f.id)).collect()
    } else {
        Default::default()
    };
    let folder = s.view.folder;
    move |m: &Message| if everywhere { !hidden.contains(&m.folder) } else { m.folder == folder }
}

/// [`rows`], for a search answered in part already: `decided` says whether each message the word
/// index answered for matches (`Session::search_words`), and `searched` whether each whose body
/// was brought in and let go of again while the search ran did (`Session::load_bodies_for_search`),
/// which a body named only can no longer say.
pub fn rows_with(s: &Session, decided: &HashMap<Id, bool>, searched: &HashMap<Id, bool>) -> Vec<Row> {
    let mb = &s.mailbox;
    let query = Query::parse(&s.view.search);
    let in_view = in_view(s);
    let me: Vec<&str> = mb.accounts.iter().map(|a| a.email.as_str()).filter(|e| !e.is_empty()).collect();
    let mut shown: Vec<&Message> = mb
        .messages
        .values()
        .map(|m| m.as_ref())
        .filter(|m| in_view(m))
        .filter(|m| match s.view.filter {
            Filter::All => true,
            Filter::Unread => !m.read,
            Filter::Flagged => m.flag == FlagStatus::Flagged,
            Filter::Attachments => m.has_attachments(),
            Filter::ToMe => m.to.iter().any(|a| me.iter().any(|e| a.email.eq_ignore_ascii_case(e))),
        })
        .filter(|m| {
            query.is_empty()
                || match decided.get(&m.id).or_else(|| searched.get(&m.id).filter(|_| m.body_is_kept())) {
                    Some(hit) => *hit,
                    None => query.matches(m, s.offset_at(m.date)),
                }
        })
        .collect();

    let desc = s.view.sort_descending;
    shown.sort_by(|a, b| {
        let key = match s.view.sort {
            SortBy::Date => when(a).cmp(&when(b)),
            // Within one sender or subject, newest first when the names run A to Z.
            SortBy::From => a.from.display().to_lowercase().cmp(&b.from.display().to_lowercase()).then(when(b).cmp(&when(a))),
            SortBy::Subject => a.topic().to_lowercase().cmp(&b.topic().to_lowercase()).then(when(b).cmp(&when(a))),
            // As the row says it (#273): the server's size for what isn't here; one not known
            // counts as the smallest.
            SortBy::Size => a.list_size().cmp(&b.list_size()),
            SortBy::Importance => a.importance.cmp(&b.importance).then(when(a).cmp(&when(b))),
            // Newest first; `by_category` then lists each under its categories.
            SortBy::Categories => when(b).cmp(&when(a)),
        };
        // Pinned messages stay on top whichever way the list is sorted; ids break ties so the
        // order never depends on the map's iteration.
        // The categories, not the dates, are what turning the list over turns over.
        let key = if desc && s.view.sort != SortBy::Categories { key.reverse() } else { key };
        b.pinned.cmp(&a.pinned).then(key).then(b.id.cmp(&a.id))
    });

    if s.view.sort == SortBy::Categories {
        return by_category(s, &shown, desc);
    }

    let mut out: Vec<Row> = Vec::new();
    if s.view.conversations && s.view.sort == SortBy::Date {
        // One row per conversation, where its newest (or, ascending, oldest) message sorts.
        let mut index: HashMap<(Id, String), usize> = HashMap::new();
        for m in &shown {
            match index.get(&(m.account, m.thread_key())) {
                Some(i) => {
                    if let Some(row) = out.get_mut(*i) {
                        row.members.push(m.id);
                        row.unread += usize::from(!m.read);
                    }
                }
                None => {
                    index.insert((m.account, m.thread_key()), out.len());
                    out.push(Row { id: m.id, members: vec![m.id], unread: usize::from(!m.read), group: None, copy: 0 });
                }
            }
        }
        for row in &mut out {
            // Newest first inside a conversation, and the row shows the newest.
            row.members.sort_by_key(|id| std::cmp::Reverse(mb.message(*id).map(|m| (when(m), m.id))));
            if let Some(first) = row.members.first() {
                row.id = *first;
            }
        }
    } else {
        out.extend(shown.iter().map(|m| Row { id: m.id, members: vec![m.id], unread: usize::from(!m.read), group: None, copy: 0 }));
    }

    let mut last: Option<String> = None;
    // Show in Groups off: the sorted rows, no headings.
    for row in out.iter_mut().filter(|_| s.prefs.show_groups) {
        let label = mb.message(row.id).and_then(|m| group_label(s, m));
        if label != last {
            row.group = label.clone();
            last = label;
        }
    }
    out
}

/// The list arranged by categories, as Outlook arranges it: a message is listed under each of its
/// categories, those with none under "No category" first, then the categories A to Z (Z to A
/// turned over), newest first under each. A pinned message is listed once, on top. `shown` is
/// sorted already: pinned first, then newest first.
///
/// Show in Groups off doesn't apply: a message is listed under each of its categories, which takes
/// the headings, so they stay. Outlook does the same: unticking it there leaves a list arranged by
/// categories grouped as it was (observed 2026-10-22).
fn by_category(s: &Session, shown: &[&Message], desc: bool) -> Vec<Row> {
    // (heading, message); the sort is stable, so under each heading `shown`'s order stays.
    let mut placed: Vec<(Option<String>, &Message)> = Vec::new();
    for m in shown {
        let names = if m.pinned { Vec::new() } else { category_names(s, &m.categories) };
        if names.is_empty() {
            placed.push((None, m));
        }
        placed.extend(names.into_iter().map(|name| (Some(name.to_string()), *m)));
    }
    placed.sort_by_cached_key(|(name, m)| {
        let key = name.as_ref().map(|n| n.to_lowercase());
        (std::cmp::Reverse(m.pinned), if desc { Err(std::cmp::Reverse(key)) } else { Ok(key) })
    });
    let mut copies: HashMap<Id, u16> = HashMap::new();
    let mut last: Option<String> = None;
    let mut out = Vec::with_capacity(placed.len());
    for (name, m) in placed {
        let copy = copies.entry(m.id).or_default();
        let mut row = Row { id: m.id, members: vec![m.id], unread: usize::from(!m.read), group: None, copy: *copy };
        *copy = copy.saturating_add(1);
        let label = Some(if m.pinned { "Pinned".to_string() } else { name.unwrap_or_else(|| NO_CATEGORY.into()) });
        if label != last {
            row.group = label.clone();
            last = label;
        }
        out.push(row);
    }
    out
}

/// The row a message is in (its first, when it is listed more than once).
pub fn position(rows: &[Row], id: Id) -> Option<usize> {
    rows.iter().position(|r| r.id == id || r.members.contains(&id))
}

/// The row numbered `copy` of a message's rows, or its first when it has fewer.
pub fn position_of(rows: &[Row], id: Id, copy: u16) -> Option<usize> {
    rows.iter().position(|r| r.id == id && r.copy == copy).or_else(|| position(rows, id))
}

/// How many messages (or conversations) the list shows: one listed under several categories
/// counts once.
pub fn listed(rows: &[Row]) -> usize {
    rows.iter().filter(|r| r.copy == 0).count()
}
