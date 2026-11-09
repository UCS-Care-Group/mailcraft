// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's inbox categories in the window (#911): the five folders the folder pane shows under a
//! Gmail account's Inbox, and the Inbox's mail each lists.
//!
//! They are no folders of the mailbox: each is a view of the Inbox (`folder.select` with
//! `gmail`), so nothing can be renamed, deleted or filed in one, and the Inbox's mail stays in
//! the Inbox. Primary, Social, Promotions, Updates and Forums are Gmail's decision, read from its
//! server ([`mailcraft_model::GmailCategory`]).
//!
//! The owner may move mail from one to another (`mail.setGmailCategory`), for an account that
//! signs in with Google ([`moves_offered`], [`why_not_movable`]): the move is made here at once,
//! like a read mark, and goes to Gmail through its API at the next Send/Receive
//! (`SyncRequest::gmail_moves`). That ships switched off until Google has reviewed MailCraft's
//! use of the API ([`GMAIL_CATEGORY_MOVES`]).

use std::collections::HashSet;
use std::sync::Arc;

use mailcraft_model::{FolderKind, GmailCategory, Id, Message, ThreadIndex, ThreadKey};

use crate::Session;

/// Whether mail can be moved between Gmail's categories in this build (#911): the model's switch,
/// which `mailcraft-net` holds to as well. [`Session::gmail_category_moves`] starts as this.
pub use mailcraft_model::gmail::GMAIL_CATEGORY_MOVES;

/// Why moves between Gmail's categories aren't there while they are switched off. (The reasons
/// are written out where they are given, so that they are translated; these are for comparing.)
pub const MOVES_NOT_YET: &str = "comes once Google has reviewed MailCraft";
/// Why an account signed in with an app password can't move mail between Gmail's categories.
pub const NEEDS_GOOGLE_SIGN_IN: &str = "needs Google sign-in";

/// Whether moving mail between Gmail's categories is offered (`mail.setGmailCategory`): `None`
/// while moves are switched on, outside the browser, for a mailbox with a Gmail account that
/// signs in with Google; else why not.
pub fn moves_offered(s: &Session) -> Option<&'static str> {
    if !s.gmail_category_moves {
        return Some("comes once Google has reviewed MailCraft");
    }
    if cfg!(target_arch = "wasm32") {
        return Some("can't be done in the browser");
    }
    let gmail = s.mailbox.accounts.iter().filter(|a| !a.is_shared() && a.is_gmail());
    match gmail.clone().any(|a| a.moves_gmail_categories()) {
        true => None,
        false if gmail.count() > 0 => Some("needs Google sign-in"),
        false => Some("needs a Gmail account"),
    }
}

/// Why message `m` can't be moved between Gmail's categories, if it can't: it isn't in a Gmail
/// Inbox Gmail sorted, its account signs in with an app password ([`NEEDS_GOOGLE_SIGN_IN`]), or
/// Gmail hasn't said its id of it yet.
pub fn why_not_movable(s: &Session, m: &Message) -> Option<&'static str> {
    let mb = &s.mailbox;
    let inbox = mb.folder(m.folder).is_some_and(|f| f.kind == FolderKind::Inbox && f.account == m.account);
    let Some(account) = mb.account(m.account).filter(|a| inbox && !a.is_shared() && a.is_gmail() && m.gmail_category.is_some()) else {
        return Some("only mail in a Gmail account's Inbox is in one of Gmail's categories");
    };
    if !account.moves_gmail_categories() {
        return Some("needs Google sign-in");
    }
    if m.gmail_id == 0 {
        return Some("Gmail hasn't said its id of this message yet: Send/Receive first");
    }
    None
}

/// Whether `folder` is the Inbox of a Gmail account whose owner didn't turn its categories off.
pub fn has_categories(s: &Session, folder: Id) -> bool {
    let mb = &s.mailbox;
    mb.folder(folder)
        .is_some_and(|f| f.kind == FolderKind::Inbox && mb.account(f.account).is_some_and(|a| !a.is_shared() && a.shows_gmail_categories()))
}

/// Whether View › Gmail Categories is offered (`view.gmailCategories`): `None` when an account of
/// the mailbox is a Gmail account whose owner didn't turn its categories off, else why not.
pub fn offered(s: &Session) -> Option<&'static str> {
    let any = s.mailbox.accounts.iter().any(|a| !a.is_shared() && a.shows_gmail_categories());
    (!any).then_some("needs a Gmail account whose categories are shown")
}

/// Whether the list in view has Gmail's categories as a row of tabs above it (#911): the setting
/// is tabs (`view.gmailCategories`), Mail is in front, and the folder in view is a Gmail Inbox
/// whose categories are listed ([`listed_under`]). The folder pane then lists none under it.
pub fn tabs_shown(s: &mut Session) -> bool {
    s.view.gmail_tabs && s.view.module == crate::Module::Mail && listed_under(s, s.view.folder)
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

/// Whether any message of `folder` is in a category other than Primary: [`listed_under`]'s test,
/// from the counts when they are up to date, else a walk over the messages.
fn sorted(s: &Session, folder: Id) -> bool {
    match s.fresh_counts() {
        Some(counts) => GmailCategory::SEARCHED.iter().any(|c| counts.gmail(folder, *c).total > 0),
        None => s.mailbox.messages.values().any(|m| m.folder == folder && m.gmail_category.is_some_and(|c| c != GmailCategory::Primary)),
    }
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
