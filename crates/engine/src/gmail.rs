// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's inbox categories in the window (#911): the five folders the folder pane shows under a
//! Gmail account's Inbox, and the Inbox's mail each lists.
//!
//! They are no folders of the mailbox: each is a view of the Inbox (`folder.select` with
//! `gmail`), so nothing can be renamed, deleted or filed in one, and the Inbox's mail stays in
//! the Inbox. Primary, Social, Promotions, Updates and Forums are Gmail's decision, read from its
//! server ([`mailcraft_model::GmailCategory`]).

use std::collections::HashSet;
use std::sync::Arc;

use mailcraft_model::{FolderKind, GmailCategory, Id, Message, ThreadIndex, ThreadKey};

use crate::Session;

/// Whether `folder` is the Inbox of a Gmail account whose owner didn't turn its categories off.
pub fn has_categories(s: &Session, folder: Id) -> bool {
    let mb = &s.mailbox;
    mb.folder(folder)
        .is_some_and(|f| f.kind == FolderKind::Inbox && mb.account(f.account).is_some_and(|a| !a.is_shared() && a.shows_gmail_categories()))
}

/// Whether the folder pane lists the five under `folder`: [`has_categories`], and Gmail put at
/// least one of its messages in a category other than Primary (an account whose owner turned
/// categories off in Gmail has everything in Primary, and nothing to sort).
pub fn listed_under(s: &mut Session, folder: Id) -> bool {
    if !has_categories(s, folder) {
        return false;
    }
    let counts = s.folder_counts();
    GmailCategory::SEARCHED.iter().any(|c| counts.gmail(folder, *c).total > 0)
}

/// The Gmail category the list in view shows (#911): the one chosen for the folder in view,
/// while the folder pane lists that folder's categories ([`listed_under`]); else the list shows
/// the whole folder, and the pane has the folder itself as the one in view.
pub fn in_view(s: &Session) -> Option<GmailCategory> {
    let (folder, category) = s.view.gmail?;
    (folder == s.view.folder && has_categories(s, folder) && sorted(s, folder)).then_some(category)
}

/// Whether any message of `folder` is in a category other than Primary: [`listed_under`] without
/// the counts, for a caller that can't count.
// ponytail: a walk over the messages until the first one sorted; [`listed_under`]'s counts if it shows up in a profile.
fn sorted(s: &Session, folder: Id) -> bool {
    s.mailbox.messages.values().any(|m| m.folder == folder && m.gmail_category.is_some_and(|c| c != GmailCategory::Primary))
}

/// What the list in view keeps of a Gmail account's Inbox, while it shows one of its categories:
/// the Inbox's messages Gmail put in it, and in conversation view (`threads`, each conversation
/// by its account and key) every message of a conversation one of them is in (spec: a
/// conversation is listed in a category when any of its messages is).
pub(crate) struct Shown {
    folder: Id,
    category: GmailCategory,
    threads: Option<(Arc<ThreadIndex>, HashSet<(Id, ThreadKey)>)>,
}

impl Shown {
    /// For the list in view; `None` when it shows no category. `messages` are the folder's,
    /// looked through for the conversations when the list groups them.
    pub(crate) fn of<'a>(s: &Session, messages: impl Iterator<Item = &'a Message>) -> Option<Shown> {
        let category = in_view(s)?;
        let folder = s.view.folder;
        let threads = s.view.conversations.then(|| {
            let index = s.threads();
            let keys = messages.filter(|m| m.folder == folder && m.gmail_category == Some(category)).map(|m| (m.account, index.key(m.id))).collect();
            (index, keys)
        });
        Some(Shown { folder, category, threads })
    }

    /// Whether the list keeps `m`: what isn't in the Inbox (a search of more than it) stays as it
    /// is; of the Inbox's, those in the category or a conversation of it.
    pub(crate) fn keeps(&self, m: &Message) -> bool {
        if m.folder != self.folder || m.gmail_category == Some(self.category) {
            return true;
        }
        self.threads.as_ref().is_some_and(|(index, keys)| !keys.is_empty() && keys.contains(&(m.account, index.key(m.id))))
    }
}
