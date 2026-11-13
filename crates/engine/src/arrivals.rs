// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! New mail in an Inbox of the owner's own accounts (R7.1, Desktop Alerts): what a Send/Receive
//! brought that is worth telling the owner of while they are elsewhere.
//!
//! The window marks the mailbox before a run ([`mark`]) and asks, once the run is over, what
//! arrived since ([`arrivals`]). Only a message that is in an Inbox of one of the owner's own
//! accounts when the run is over, unread, and was not in the mailbox before counts. So these
//! never do:
//!
//! - an account's first download (its Inbox had never been read from its server when the run
//!   began: no UID seen, or for a Microsoft account no round of the folder ever ended; a run that
//!   stopped partway through a later round does not make it one again, #609);
//! - mail a rule moved out of the Inbox as it arrived, and mail in Junk E-mail or any other
//!   folder;
//! - the mail of a shared mailbox, of a local account, and of an account a newer MailCraft
//!   added, which no run here keeps in step (nothing arrives in either by itself);
//! - an Inbox the server numbered anew during the run (UIDVALIDITY changed), whose mail all comes
//!   down again;
//! - mail already read (on another device, or by a rule);
//! - for a Gmail account whose owner asked for alerts for Primary alone (#911), mail Gmail put in
//!   another of its categories.
//!
//! [`new_inbox_count`] is the same mail, counted for the taskbar icon, with two differences: a
//! shared mailbox's Inbox counts too, and a Gmail account's other categories count too. Junk,
//! another folder, a first download and mail already read still don't.
//!
//! Nothing here shows anything: it only says which messages, and who they are from. The text is
//! the message's own, hostile, and the window cleans it before it goes anywhere.

use mailcraft_model::{FolderKind, Id, Mailbox};

/// The mailbox as a run found it ([`mark`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxMark {
    /// The highest message id there was: a message with a higher one was added since (ids are
    /// handed out in increasing order and never again).
    last_id: Id,
    /// The Inboxes a server keeps, a shared mailbox's included, that had been read from their
    /// server, each with the UIDVALIDITY it had (0 for a Microsoft account's). Desktop alerts
    /// still count only the owner's own.
    inboxes: Vec<(Id, u32)>,
}

/// A message that arrived ([`arrivals`]): its text as the message has it, not cleaned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub message: Id,
    pub account: Id,
    /// The Inbox it is in.
    pub folder: Id,
    /// Who it is from: the sender's name, or the address when there is no name.
    pub sender: String,
    pub subject: String,
}

/// How many arrivals one run reports at most: the window shows a few, and only counts the rest.
pub const MOST: usize = 1_000;

/// Mark the mailbox before a run.
pub fn mark(mailbox: &Mailbox) -> InboxMark {
    let last_id = mailbox.messages.keys().next_back().copied().unwrap_or(0);
    let inboxes = mailbox
        .folders
        .iter()
        .filter(|f| f.kind == FolderKind::Inbox && on_a_server(mailbox, f.account))
        .filter(|f| f.uid_seen > 0 || f.server.as_ref().is_some_and(|s| s.round_ended || s.caught_up))
        .map(|f| (f.id, f.uid_validity))
        .collect();
    InboxMark { last_id, inboxes }
}

/// An account a server keeps mail for, and a run here keeps in step: not one that lives in this
/// mailbox only, and not one a newer MailCraft added. A shared mailbox is one.
fn on_a_server(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.account(account).is_some_and(|a| a.kind.has_server())
}

/// An account of the owner's own that a server keeps mail for ([`on_a_server`], and not a shared
/// mailbox). Desktop alerts are for these alone.
fn own(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.account(account).is_some_and(|a| !a.is_shared() && a.kind.has_server())
}

/// An Inbox that was read from its server before `before`, and that the server didn't number anew.
fn watched(mailbox: &Mailbox, before: &InboxMark, folder: Id) -> bool {
    let Some((_, validity)) = before.inboxes.iter().find(|(id, _)| *id == folder) else { return false };
    mailbox.folder(folder).is_some_and(|f| f.kind == FolderKind::Inbox && (*validity == 0 || f.uid_validity == *validity))
}

/// Unread mail added since `before`, still in an Inbox a server keeps (not Junk, not another
/// folder, not a draft, not a first download).
fn is_new(mailbox: &Mailbox, before: &InboxMark, m: &mailcraft_model::Message) -> bool {
    !m.read && !m.draft && watched(mailbox, before, m.folder) && on_a_server(mailbox, m.account)
}

/// What arrived in the Inboxes since `before` was marked, oldest first; at most [`MOST`].
pub fn arrivals(mailbox: &Mailbox, before: &InboxMark) -> Vec<Arrival> {
    let Some(from) = before.last_id.checked_add(1) else { return Vec::new() };
    mailbox
        .messages
        .range(from..)
        .map(|(_, m)| m)
        .filter(|m| is_new(mailbox, before, m) && own(mailbox, m.account) && alerted(mailbox, m))
        .take(MOST)
        .map(|m| Arrival { message: m.id, account: m.account, folder: m.folder, sender: m.from.display().to_string(), subject: m.subject.clone() })
        .collect()
}

/// How many new messages [`arrivals`] would see in every Inbox a server keeps, a shared mailbox's
/// included and every Gmail category included. At most [`MOST`].
pub fn new_inbox_count(mailbox: &Mailbox, before: &InboxMark) -> usize {
    let Some(from) = before.last_id.checked_add(1) else { return 0 };
    mailbox.messages.range(from..).map(|(_, m)| m).filter(|m| is_new(mailbox, before, m)).take(MOST).count()
}

/// Whether `m` is mail its account alerts for: any, but for a Gmail account that alerts for
/// Primary alone (#911), mail Gmail put in another category. One whose category isn't known yet
/// is alerted for: better told once too often than not at all.
fn alerted(mailbox: &Mailbox, m: &mailcraft_model::Message) -> bool {
    let primary_only = mailbox.account(m.account).is_some_and(|a| a.is_gmail() && a.alert_primary_only);
    !primary_only || m.gmail_category.is_none_or(|c| c == mailcraft_model::GmailCategory::Primary)
}

#[cfg(test)]
#[path = "arrivals_tests.rs"]
mod tests;
