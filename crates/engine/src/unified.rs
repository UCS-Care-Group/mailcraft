// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! All Accounts (#625): every account's Inbox (Drafts, Sent Items, Deleted Items, Archive) in one
//! list. A view over the accounts' own folders: no message is copied, and what is done to one
//! (read, flag, move, delete, reply) is done to it in its own account, so each account's
//! synchronisation is what it would be had the owner opened that account's folder.

use mailcraft_model::{FolderKind, Id, Mailbox};

use crate::Session;

/// The kinds of folder All Accounts offers, in the order the folder pane lists them.
pub const KINDS: [FolderKind; 5] = [FolderKind::Inbox, FolderKind::Drafts, FolderKind::Sent, FolderKind::Deleted, FolderKind::Archive];

/// The folders of `kind` the list shows: each of the owner's own accounts' top-level one, in the
/// folder pane's order. A shared mailbox is someone else's mail and stays out.
pub fn folders(mb: &Mailbox, kind: FolderKind) -> Vec<Id> {
    mb.accounts_in_order().into_iter().filter(|a| !a.is_shared()).filter_map(|a| mb.folder_of_kind(a.id, kind)).collect()
}

/// Whether the folder pane offers All Accounts: the owner has two accounts of their own at least.
pub fn offered(mb: &Mailbox) -> bool {
    mb.accounts.iter().filter(|a| !a.is_shared()).take(2).count() == 2
}

/// The kind of folder the list shows of every account, when it shows one: while the folder in
/// view is a top-level folder of that kind (anything that puts another folder in view leaves it).
pub fn of(s: &Session) -> Option<FolderKind> {
    let kind = s.view.unified?;
    s.mailbox.folder(s.view.folder).is_some_and(|f| f.kind == kind && f.parent.is_none()).then_some(kind)
}

/// The folders the list in view shows instead of the folder in view, while it shows every
/// account's ([`of`]).
pub fn in_view(s: &Session) -> Option<Vec<Id>> {
    of(s).map(|kind| folders(&s.mailbox, kind))
}

/// The folder that stands in view for every account's of `kind` ([`of`]): the default account's,
/// else the first that has one.
pub(crate) fn anchor(mb: &Mailbox, kind: FolderKind) -> Option<Id> {
    mb.default_account().filter(|a| !a.is_shared()).and_then(|a| mb.folder_of_kind(a.id, kind)).or_else(|| folders(mb, kind).first().copied())
}
