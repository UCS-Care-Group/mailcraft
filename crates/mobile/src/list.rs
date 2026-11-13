// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list the phone shows: the windowed query with everything the list's bar offers
//! (filter, Inbox split tab, conversations, search), and the row of tabs of an Inbox split.
//! Read-only: the session's own view (the folder in view, its filter, search and selection) is
//! as it was when the query is answered, so a list drawn here never moves the desktop's state.

use mailcraft_engine::{Filter, Scope, Session};
use mailcraft_model::GmailCategory;
use serde_json::Value;

use crate::queries::row;
use crate::records::{CategoryList, CategoryRecord, InboxSplit, ListQuery, MessageWindow, SplitTab};
use crate::text;
use crate::{Core, CoreError};

/// The longest search text read; more is cut.
const MAX_SEARCH: usize = 256;

fn bad(message: &str) -> CoreError {
    CoreError::BadRequest { reason: message.to_string() }
}

/// The window of `count` rows from `offset` of the list `q` asks for; `conversations` is
/// whether it folds conversations (`None`: as the session's own view does).
pub(crate) fn window(s: &mut Session, q: &ListQuery, conversations: Option<bool>, offset: u32, count: usize) -> Result<MessageWindow, CoreError> {
    let filter: Filter = serde_json::from_value(Value::String(q.filter.clone())).map_err(|_| bad("that filter is not known"))?;
    if s.mailbox.folder(q.folder).is_none() {
        return Err(bad("there is no such folder"));
    }
    let split = if q.split.is_empty() {
        None
    } else {
        let category = GmailCategory::parse(&q.split).ok_or_else(|| bad("that Inbox split tab is not known"))?;
        if !mailcraft_engine::gmail::has_categories(s, q.folder) {
            return Err(bad("that folder has no Inbox split"));
        }
        Some((q.folder, category))
    };
    // The list is built by the engine's non-mutating path over a view made for this query alone
    // (`Session::rows` would also move the selection's stays and the list cache of the folder in
    // view), and the view is put back after.
    let saved = s.view.clone();
    s.view.folder = q.folder;
    s.view.filter = filter;
    s.view.search = text::clip(q.search.trim(), MAX_SEARCH);
    s.view.scope = if q.everywhere { Scope::Everywhere } else { Scope::Folder };
    s.view.gmail = split;
    if let Some(on) = conversations {
        s.view.conversations = on;
    }
    s.view.open.clear();
    let rows = mailcraft_engine::list::rows(s);
    s.view = saved;
    let start = usize::try_from(offset).unwrap_or(usize::MAX);
    let window =
        rows.iter().skip(start).take(count).filter_map(|r| s.mailbox.message(r.id).map(|m| row(m, &r.members, r.unread, r.group.clone()))).collect();
    Ok(MessageWindow { rev: s.rev(), folder: q.folder, total: text::count_of(rows.len()), offset, rows: window })
}

#[uniffi::export]
impl Core {
    /// A window of the message list `query` asks for: `count` rows (1 to 200) from row `offset`,
    /// stamped with the revision it was read at. A search reads what is on this phone: the
    /// sender, the subject and the preview of each message (what is not fetched yet has its
    /// headers and preview only).
    pub async fn message_list(&self, query: ListQuery, offset: u32, count: u32) -> Result<MessageWindow, CoreError> {
        let count = text::window(count);
        self.call(move |s| window(s, &query, Some(query.conversations), offset, count)).await
    }

    /// The categories mail of `account` can be given (0: the list shared by accounts that keep
    /// none of their own), with their colours.
    pub async fn categories(&self, account: u64) -> Result<CategoryList, CoreError> {
        self.call(move |s| {
            let categories = text::first(s.mailbox.categories_of(account), text::MAX_LISTED, |c| CategoryRecord {
                name: text::clip(&c.name, text::MAX_NAME),
                color: (u32::from(c.color.0) << 16) | (u32::from(c.color.1) << 8) | u32::from(c.color.2),
            });
            Ok(CategoryList { rev: s.rev(), categories })
        })
        .await
    }

    /// The tabs over `folder` when it is an Inbox its server divides (Gmail's categories, for an
    /// account that shows them and whose mail Gmail sorted), with their counts; no tabs else.
    pub async fn inbox_split(&self, folder: u64) -> Result<InboxSplit, CoreError> {
        self.call(move |s| {
            if s.mailbox.folder(folder).is_none() {
                return Err(bad("there is no such folder"));
            }
            let rev = s.rev();
            if !mailcraft_engine::gmail::listed_under(s, folder) {
                return Ok(InboxSplit { rev, folder, tabs: Vec::new() });
            }
            let counts = s.folder_counts();
            let tabs = GmailCategory::ALL
                .iter()
                .map(|c| {
                    let held = counts.gmail(folder, *c);
                    SplitTab {
                        code: c.code().to_string(),
                        name: c.name().to_string(),
                        unread: text::count_of(held.unread),
                        total: text::count_of(held.total),
                    }
                })
                .collect();
            Ok(InboxSplit { rev, folder, tabs })
        })
        .await
    }
}
