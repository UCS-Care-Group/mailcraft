// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list: which messages show for the folder, filter and search in view, in what
//! order, under which group headings.

use std::collections::{HashMap, HashSet};

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
    /// Whether the heading is one of the owner's categories (to be drawn as spelt, never
    /// translated) and not a date, importance or one of the program's own (`Pinned`, no category).
    pub category: bool,
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

/// How a category name is compared: Unicode lower case, for sorting, for telling two spellings
/// apart and for its heading alike.
pub fn fold(name: &str) -> String {
    name.to_lowercase()
}

/// One spelling for each category name however it was written, by its [`fold`]: the mailbox's
/// list of categories first, then the first spelling met among `items`' categories. One map per
/// list build, so no name is searched for item by item.
pub fn category_spellings<'a>(s: &'a Session, items: impl IntoIterator<Item = &'a [String]>) -> HashMap<String, &'a str> {
    let mut spelling: HashMap<String, &str> = s.mailbox.categories.iter().map(|k| (fold(&k.name), k.name.as_str())).collect();
    for categories in items {
        for c in categories.iter().map(|c| c.trim()).filter(|c| !c.is_empty()) {
            spelling.entry(fold(c)).or_insert(c);
        }
    }
    spelling
}

/// An item's categories one by one, A to Z, each once whatever capitals it was given in (folded
/// names, see [`fold`]): the headings an item arranged by categories is listed under.
pub fn category_names(categories: &[String]) -> Vec<String> {
    let mut names: Vec<String> = categories.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).map(fold).collect();
    names.sort();
    names.dedup();
    names
}

pub fn rows(s: &Session) -> Vec<Row> {
    rows_with(s, &HashMap::new(), &HashMap::new())
}

/// Which messages the list looks at: those of the folder in view; for a search of its
/// subfolders, those of the folder and every folder under it; for a search of the mailbox in
/// view, those of every folder of its account; for a search of every mailbox, every message. A
/// search of the mailbox or of every mailbox leaves out Deleted Items, Junk E-mail and their
/// subfolders (#493).
pub fn in_view(s: &Session) -> impl Fn(&Message) -> bool + '_ {
    let mb = &s.mailbox;
    let folder = s.view.folder;
    let scope = if Query::parse(&s.view.search).is_empty() { Scope::Folder } else { s.view.scope };
    // Deleted and junk mail stays out of a search of a whole mailbox (`account`) or of all.
    let hidden = |account: Option<Id>| -> HashSet<Id> {
        mb.folders
            .iter()
            .filter(|f| account.is_none_or(|a| f.account == a) && matches!(f.kind, FolderKind::Deleted | FolderKind::Junk))
            .flat_map(|f| mb.subtree(f.id))
            .collect()
    };
    // The folders looked in, or (for every mailbox) the folders left out.
    let (everywhere, folders): (bool, HashSet<Id>) = match scope {
        Scope::Folder => (false, HashSet::from([folder])),
        Scope::Subfolders => (false, mb.subtree(folder).into_iter().collect()),
        Scope::Mailbox => {
            let account = mb.folder(folder).map(|f| f.account);
            let hidden = hidden(account);
            let of_account = mb.folders.iter().filter(|f| account == Some(f.account) && !hidden.contains(&f.id));
            (false, of_account.map(|f| f.id).collect())
        }
        Scope::Everywhere => (true, hidden(None)),
    };
    move |m: &Message| folders.contains(&m.folder) != everywhere
}

/// [`rows`], for a search answered in part already: `decided` says whether each message the word
/// index answered for matches (`Session::search_words`), and `searched` whether each whose body
/// was brought in and let go of again while the search ran did (`Session::load_bodies_for_search`),
/// which a body named only can no longer say.
pub fn rows_with(s: &Session, decided: &HashMap<Id, bool>, searched: &HashMap<Id, bool>) -> Vec<Row> {
    listing(s, decided, searched).0
}

/// The addresses of the owner's accounts, for To Me.
fn me(s: &Session) -> Vec<&str> {
    s.mailbox.accounts.iter().map(|a| a.email.as_str()).filter(|e| !e.is_empty()).collect()
}

/// Whether `m` matches the filter in view (All, Unread, Flagged…); `me` as from [`me`].
fn filtered(s: &Session, m: &Message, me: &[&str]) -> bool {
    match s.view.filter {
        Filter::All => true,
        Filter::Unread => !m.read,
        Filter::Flagged => m.flag == FlagStatus::Flagged,
        Filter::Attachments => m.has_attachments(),
        Filter::ToMe => m.to.iter().any(|a| me.iter().any(|e| a.email.eq_ignore_ascii_case(e))),
    }
}

/// Of `ids`, those the list in view lists now, without building it (#428): in the folder in view,
/// and matching the filter or kept. For a list with no search: what a search finds, only the
/// list can say.
pub(crate) fn lists(s: &Session, ids: &[Id]) -> std::collections::HashSet<Id> {
    let in_view = in_view(s);
    let me = me(s);
    let kept = s.kept();
    let shows = |m: &Message| in_view(m) && (filtered(s, m, &me) || kept.as_ref().is_some_and(|kept| kept.contains(&m.id)));
    ids.iter().copied().filter(|id| s.mailbox.message(*id).is_some_and(shows)).collect()
}

/// [`rows_with`], and the messages the rows list only because they are selected (#428): they were
/// in this list when they were selected and no longer match the filter or the search.
pub(crate) fn listing(s: &Session, decided: &HashMap<Id, bool>, searched: &HashMap<Id, bool>) -> (Vec<Row>, Vec<Id>) {
    let mb = &s.mailbox;
    let query = Query::parse(&s.view.search);
    let in_view = in_view(s);
    let me = me(s);
    let kept = s.kept();
    let mut kept_only = Vec::new();
    let mut shown: Vec<&Message> = mb
        .messages
        .values()
        .map(|m| m.as_ref())
        .filter(|m| in_view(m))
        .filter(|m| {
            let hit = filtered(s, m, &me)
                && (query.is_empty()
                    || match decided.get(&m.id).or_else(|| searched.get(&m.id).filter(|_| m.body_is_kept())) {
                        Some(hit) => *hit,
                        None => query.matches(m, s.offset_at(m.date)),
                    });
            // A selected message stays while it is selected, though it was read or its flag was
            // cleared; `in_view` above still applies, so one deleted, or moved out of the folder in
            // view, is gone.
            if !hit && kept.as_ref().is_some_and(|kept| kept.contains(&m.id)) {
                kept_only.push(m.id);
                return true;
            }
            hit
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
        return (by_category(s, &shown, desc), kept_only);
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
                    out.push(Row { id: m.id, members: vec![m.id], unread: usize::from(!m.read), group: None, category: false, copy: 0 });
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
        out.extend(shown.iter().map(|m| Row { id: m.id, members: vec![m.id], unread: usize::from(!m.read), group: None, category: false, copy: 0 }));
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
    (out, kept_only)
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
    let spelling = category_spellings(s, shown.iter().map(|m| m.categories.as_slice()));
    // (heading's folded name, message); the sort is stable, so under each heading `shown`'s order stays.
    let mut placed: Vec<(Option<String>, &Message)> = Vec::new();
    for m in shown {
        let names = if m.pinned { Vec::new() } else { category_names(&m.categories) };
        if names.is_empty() {
            placed.push((None, m));
        }
        placed.extend(names.into_iter().map(|name| (Some(name), *m)));
    }
    placed.sort_by_cached_key(|(name, m)| (std::cmp::Reverse(m.pinned), if desc { Err(std::cmp::Reverse(name.clone())) } else { Ok(name.clone()) }));
    let mut copies: HashMap<Id, u16> = HashMap::new();
    let mut last: Option<(String, bool)> = None;
    let mut out = Vec::with_capacity(placed.len());
    for (name, m) in placed {
        let copy = copies.entry(m.id).or_default();
        let mut row = Row { id: m.id, members: vec![m.id], unread: usize::from(!m.read), group: None, category: false, copy: *copy };
        *copy = copy.saturating_add(1);
        let (label, category) = if m.pinned {
            ("Pinned".to_string(), false)
        } else {
            match name {
                Some(n) => (spelling.get(&n).map_or(n.clone(), |k| k.to_string()), true),
                None => (NO_CATEGORY.to_string(), false),
            }
        };
        let key = (fold(&label), category);
        if last.as_ref() != Some(&key) {
            row.group = Some(label);
            row.category = category;
            last = Some(key);
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
