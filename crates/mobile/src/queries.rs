// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The typed, windowed, rev-stamped queries for bulk reads. Each is thin over what the session
//! and the model already say; none changes the mailbox (a command does, through `execute`).
//! Every list is windowed and every string cut ([`crate::text`]), so what comes back is bounded
//! whatever the mailbox holds.

use std::collections::BTreeMap;

use mailcraft_engine::cmd::calendar::occurrences;
use mailcraft_engine::{Filter, Session};
use mailcraft_model::{Address, Contact, Event, FolderKind, Id, Importance, Message, Task};
use mailcraft_time::Date;
use serde_json::Value;

use crate::records::{
    AccountRecord, AddressRecord, CalendarRange, EventRecord, FolderList, FolderRecord, MessageRow, MessageWindow, PeopleWindow, PersonRecord,
    TaskListRecord, TaskLists, TaskRecord, TaskWindow, TodayRecord,
};
use crate::text::{
    self, MAX_ADDRESS, MAX_CATEGORIES, MAX_FOLDERS, MAX_LISTED, MAX_LOCATION, MAX_NAME, MAX_PEOPLE_IN_ITEM, MAX_PREVIEW, MAX_SUBJECT, MAX_TITLE,
};
use crate::{Core, CoreError};

fn bad(message: &str) -> CoreError {
    CoreError::BadRequest { message: message.to_string() }
}

fn rgb(c: mailcraft_model::Rgb) -> u32 {
    (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)
}

fn code<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

pub(crate) fn address(a: &Address) -> AddressRecord {
    AddressRecord { name: text::clip(&a.name, MAX_NAME), address: text::clip(&a.email, MAX_ADDRESS) }
}

fn categories(c: &[String]) -> Vec<String> {
    text::first(c, MAX_CATEGORIES, |c| text::clip(c, MAX_NAME))
}

fn event_record(s: &Session, e: &Event, start: String, end: String) -> EventRecord {
    let color = s.mailbox.calendars.iter().find(|c| c.id == e.calendar).map_or(0, |c| rgb(c.color));
    EventRecord {
        event: e.id,
        calendar: e.calendar,
        color,
        title: text::clip(&e.title, MAX_TITLE),
        location: text::clip(&e.location, MAX_LOCATION),
        start,
        end,
        all_day: e.all_day,
        repeats: e.recurrence.is_some(),
        show_as: code(&e.show_as),
        meeting: e.is_meeting(),
        cancelled: e.cancelled,
        categories: categories(&e.categories),
    }
}

fn events_between(s: &Session, from: Date, to: Date) -> Vec<EventRecord> {
    occurrences(s, from, to)
        .into_iter()
        .take(text::MAX_OCCURRENCES)
        .filter_map(|o| s.mailbox.event(o.event).map(|e| event_record(s, e, o.start.iso(), o.end.iso())))
        .collect()
}

fn task_record(t: &Task) -> TaskRecord {
    TaskRecord {
        id: t.id,
        title: text::clip(&t.title, MAX_TITLE),
        due: t.due.map(Date::iso),
        start: t.start.map(Date::iso),
        completed: t.is_complete(),
        important: t.importance == Importance::High,
        account: t.origin.account,
        list: text::clip(&t.list, MAX_NAME),
        categories: categories(&t.categories),
        message: t.message,
    }
}

fn row(m: &Message, members: usize, group: Option<String>) -> MessageRow {
    MessageRow {
        id: m.id,
        account: m.account,
        folder: m.folder,
        from_name: text::clip(&m.from.name, MAX_NAME),
        from_address: text::clip(&m.from.email, MAX_ADDRESS),
        subject: text::clip(&m.subject, MAX_SUBJECT),
        preview: mailcraft_engine::list::preview(m, MAX_PREVIEW),
        when: mailcraft_engine::list::when(m),
        unread: !m.read,
        flagged: m.flag == mailcraft_model::FlagStatus::Flagged,
        has_attachments: m.attachments.iter().any(|a| !a.inline),
        important: m.importance == Importance::High,
        answered: m.answered,
        forwarded: m.forwarded,
        draft: m.draft,
        pinned: m.pinned,
        categories: categories(&m.categories),
        members: text::count_of(members),
        group: group.map(|g| text::clip(&g, MAX_NAME)),
    }
}

fn person(c: &Contact) -> PersonRecord {
    PersonRecord {
        id: c.id,
        name: text::clip(&c.name(), MAX_NAME),
        company: text::clip(&c.company, MAX_NAME),
        title: text::clip(&c.title, MAX_NAME),
        emails: text::first(&c.emails, MAX_PEOPLE_IN_ITEM, |e| text::clip(&e.value, MAX_ADDRESS)),
        phones: text::first(&c.phones, MAX_PEOPLE_IN_ITEM, |p| text::clip(&p.value, MAX_NAME)),
        favorite: c.favorite,
    }
}

fn date_of(s: &str) -> Result<Date, CoreError> {
    Date::parse_iso(s).ok_or_else(|| bad("a date is written YYYY-MM-DD"))
}

#[uniffi::export]
impl Core {
    /// The folders of `account` (0 for every account), with the accounts they belong to, their
    /// message counts and their unread counts.
    pub async fn folders(&self, account: u64) -> Result<FolderList, CoreError> {
        self.call(move |s| {
            let counts = s.folder_counts();
            let wanted = |id: Id| account == 0 || id == account;
            let accounts = text::first(s.mailbox.accounts.iter().filter(|a| wanted(a.id)), MAX_LISTED, |a| AccountRecord {
                id: a.id,
                name: text::clip(&a.name, MAX_NAME),
                email: text::clip(&a.email, MAX_ADDRESS),
                display_name: text::clip(&a.display_name, MAX_NAME),
            });
            let folders = text::first(s.mailbox.folders.iter().filter(|f| wanted(f.account)), MAX_FOLDERS, |f| FolderRecord {
                id: f.id,
                account: f.account,
                parent: f.parent,
                name: text::clip(&f.name, MAX_NAME),
                kind: code(&f.kind),
                favorite: f.favorite,
                total: text::count_of(counts.total(f.id)),
                unread: text::count_of(counts.unread(f.id)),
            });
            Ok(FolderList { rev: s.rev(), accounts, folders })
        })
        .await
    }

    /// A window of the message list of `folder`, filtered by `filter` (`all`, `unread`,
    /// `flagged`, `attachments`, `toMe`), in the order and grouping the session's view has:
    /// `count` rows (1 to 200) from row `offset`. It reads the list without leaving the view
    /// changed: the folder in view, its filter, its search, the selection and the list kept for it are as they were.
    pub async fn messages(&self, folder: u64, filter: String, offset: u32, count: u32) -> Result<MessageWindow, CoreError> {
        let filter: Filter = serde_json::from_value(Value::String(filter)).map_err(|_| bad("that filter is not known"))?;
        let count = text::window(count);
        self.call(move |s| {
            if s.mailbox.folder(folder).is_none() {
                return Err(bad("there is no such folder"));
            }
            // The list is built by the engine's non-mutating path, over a view made for this
            // query alone: `Session::rows` would also move the selection's stays and the list
            // cache of the folder in view.
            let saved = s.view.clone();
            s.view.folder = folder;
            s.view.filter = filter;
            s.view.search.clear();
            s.view.gmail = None;
            s.view.scope = mailcraft_engine::Scope::Folder;
            s.view.open.clear();
            let rows = mailcraft_engine::list::rows(s);
            s.view = saved;
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            let window = rows
                .iter()
                .skip(start)
                .take(count)
                .filter_map(|r| s.mailbox.message(r.id).map(|m| row(m, r.members.len(), r.group.clone())))
                .collect();
            Ok(MessageWindow { rev: s.rev(), folder, total: text::count_of(rows.len()), offset, rows: window })
        })
        .await
    }

    /// What happens on the days `from` to `to` (`YYYY-MM-DD`, at most 100 days) in the calendars
    /// that are shown, in order; a series gives one record for each time it happens.
    pub async fn calendar(&self, from: String, to: String) -> Result<CalendarRange, CoreError> {
        let (from, to) = (date_of(&from)?, date_of(&to)?);
        if to < from || from.days_until(to) >= text::MAX_RANGE_DAYS {
            return Err(bad("the range is empty or longer than 100 days"));
        }
        self.call(move |s| Ok(CalendarRange { rev: s.rev(), from: from.iso(), to: to.iso(), events: events_between(s, from, to) })).await
    }

    /// A window of the people (contacts), by name, whose name, company or address holds
    /// `search` (any case; empty for everyone).
    pub async fn people(&self, search: String, offset: u32, count: u32) -> Result<PeopleWindow, CoreError> {
        let count = text::window(count);
        self.call(move |s| {
            let needle = search.trim().to_lowercase();
            let hit = |c: &Contact| {
                needle.is_empty()
                    || c.name().to_lowercase().contains(&needle)
                    || c.company.to_lowercase().contains(&needle)
                    || c.emails.iter().any(|e| e.value.to_lowercase().contains(&needle))
            };
            let mut found: Vec<(String, &Contact)> =
                s.mailbox.contacts.values().map(|c| &**c).filter(|c| hit(c)).map(|c| (c.name().to_lowercase(), c)).collect();
            found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            let people = found.iter().skip(start).take(count).map(|(_, c)| person(c)).collect();
            Ok(PeopleWindow { rev: s.rev(), total: text::count_of(found.len()), offset, people })
        })
        .await
    }

    /// The To Do lists, one for each account's list (and one for the phone's own tasks), with
    /// how many tasks are open and how many there are.
    pub async fn task_lists(&self) -> Result<TaskLists, CoreError> {
        self.call(|s| {
            let mut lists: BTreeMap<(Id, String), (u32, u32)> = BTreeMap::new();
            for t in s.mailbox.tasks.values() {
                let entry = lists.entry((t.origin.account, text::clip(&t.list, MAX_NAME))).or_default();
                entry.1 = entry.1.saturating_add(1);
                if !t.is_complete() {
                    entry.0 = entry.0.saturating_add(1);
                }
            }
            let lists = text::first(lists, MAX_LISTED, |((account, list), (open, total))| TaskListRecord { account, list, open, total });
            Ok(TaskLists { rev: s.rev(), lists })
        })
        .await
    }

    /// A window of the tasks of one list (`account` and `list` as `task_lists` gives them), by
    /// due date, those with none last; completed ones only when `completed` is set.
    pub async fn tasks(&self, account: u64, list: String, completed: bool, offset: u32, count: u32) -> Result<TaskWindow, CoreError> {
        let count = text::window(count);
        self.call(move |s| {
            let mut found: Vec<&Task> = s
                .mailbox
                .tasks
                .values()
                .map(|t| &**t)
                .filter(|t| t.origin.account == account && text::clip(&t.list, MAX_NAME) == list && (completed || !t.is_complete()))
                .collect();
            found.sort_by_key(|t| (t.due.is_none(), t.due, t.id));
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            let tasks = found.iter().skip(start).take(count).map(|t| task_record(t)).collect();
            Ok(TaskWindow { rev: s.rev(), total: text::count_of(found.len()), offset, tasks })
        })
        .await
    }

    /// Today: what is on the calendar, the tasks due or overdue, and the unread count of the
    /// Inboxes.
    pub async fn today(&self) -> Result<TodayRecord, CoreError> {
        self.call(|s| {
            let today = s.today();
            let events = events_between(s, today, today);
            let mut due: Vec<&Task> =
                s.mailbox.tasks.values().map(|t| &**t).filter(|t| !t.is_complete() && t.due.is_some_and(|d| d <= today)).collect();
            due.sort_by_key(|t| (t.due, t.id));
            let tasks = text::first(due, MAX_LISTED, task_record);
            let counts = s.folder_counts();
            let unread =
                s.mailbox.folders.iter().filter(|f| f.kind == FolderKind::Inbox).map(|f| counts.unread(f.id)).fold(0usize, usize::saturating_add);
            Ok(TodayRecord { rev: s.rev(), date: today.iso(), events, tasks, unread: text::count_of(unread) })
        })
        .await
    }
}
