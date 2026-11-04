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
//! - mail already read (on another device, or by a rule).
//!
//! But a message a rule that shows a desktop alert acted on as it arrived counts wherever the
//! rule put it, read or not ([`Arrival::rule`]; `Message::rule_alert`), as long as its account's
//! Inbox had been read from its server before: an account's first download tells of nothing.
//! So does one such a rule acted on only once it was fetched whole, after the run that brought it
//! (#566): the fetch names it, and [`alerted`] says who it is from.
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
    /// The Inboxes of the owner's own accounts that had been read from their server, each with
    /// the UIDVALIDITY it had (0 for a Microsoft account's).
    inboxes: Vec<(Id, u32)>,
}

/// A message that arrived ([`arrivals`]): its text as the message has it, not cleaned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub message: Id,
    pub account: Id,
    /// The folder it is in: an Inbox, or where a rule that asked for its alert put it.
    pub folder: Id,
    /// Who it is from: the sender's name, or the address when there is no name.
    pub sender: String,
    pub subject: String,
    /// A rule asked for its alert (Outlook's "display a Desktop Alert"): it is told of when new
    /// mail isn't, as in Outlook, where the owner who wants alerts for some mail alone turns them
    /// off and has a rule show them.
    pub rule: bool,
}

/// How many arrivals one run reports at most: the window shows a few, and only counts the rest.
pub const MOST: usize = 1_000;

/// Mark the mailbox before a run.
pub fn mark(mailbox: &Mailbox) -> InboxMark {
    let last_id = mailbox.messages.keys().next_back().copied().unwrap_or(0);
    let inboxes = mailbox
        .folders
        .iter()
        .filter(|f| f.kind == FolderKind::Inbox && own(mailbox, f.account))
        .filter(|f| f.uid_seen > 0 || f.server.as_ref().is_some_and(|s| s.round_ended || s.caught_up))
        .map(|f| (f.id, f.uid_validity))
        .collect();
    InboxMark { last_id, inboxes }
}

/// An account of the owner's own that a server keeps mail for, and a run here keeps in step:
/// there, not a shared mailbox, not one that lives in this mailbox only, and not one a newer
/// MailCraft added.
fn own(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.account(account).is_some_and(|a| !a.is_shared() && a.kind.has_server())
}

/// What arrived in the Inboxes since `before` was marked, oldest first; at most [`MOST`].
pub fn arrivals(mailbox: &Mailbox, before: &InboxMark) -> Vec<Arrival> {
    let Some(from) = before.last_id.checked_add(1) else { return Vec::new() };
    // An Inbox that was read from its server before, and that the server didn't number anew.
    let watched = |folder: Id| -> bool {
        let Some((_, validity)) = before.inboxes.iter().find(|(id, _)| *id == folder) else { return false };
        mailbox.folder(folder).is_some_and(|f| f.kind == FolderKind::Inbox && (*validity == 0 || f.uid_validity == *validity))
    };
    // An account whose Inbox was read before: a rule's alert for its mail is told of.
    let known = |account: Id| before.inboxes.iter().any(|(id, _)| mailbox.folder(*id).is_some_and(|f| f.account == account));
    mailbox
        .messages
        .range(from..)
        .map(|(_, m)| &**m)
        .filter(|m| !m.draft && own(mailbox, m.account) && ((!m.read && watched(m.folder)) || (m.rule_alert && known(m.account))))
        .take(MOST)
        .map(arrival)
        .collect()
}

/// Of `ids`, the messages a rule shows a desktop alert for that are still here, in the owner's
/// own accounts, in the order given; at most [`MOST`]. (A fetch names them: a rule that waited for
/// them to be here whole acted on them, [`crate::cmd::graph_mail::apply_graph_fetch`].)
pub fn alerted(mailbox: &Mailbox, ids: &[Id]) -> Vec<Arrival> {
    ids.iter().filter_map(|id| mailbox.message(*id)).filter(|m| m.rule_alert && !m.draft && own(mailbox, m.account)).take(MOST).map(arrival).collect()
}

fn arrival(m: &mailcraft_model::Message) -> Arrival {
    Arrival {
        message: m.id,
        account: m.account,
        folder: m.folder,
        sender: m.from.display().to_string(),
        subject: m.subject.clone(),
        rule: m.rule_alert,
    }
}

#[cfg(test)]
#[path = "arrivals_tests.rs"]
mod tests;
