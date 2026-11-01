// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Changes of messages up through Microsoft Graph (R3.3; R3 spec S16–S22, ADR 0003).
//!
//! What goes up is the difference between each message of a Microsoft account and its last
//! synced state ([`ServerMessage`]): read, flag (with its dates), categories and importance as one
//! change of the message, a different folder as a move. What can't be worked out by comparing (a
//! copy, a delete for good) waits in the mailbox's queue, and so does nothing else: a crash
//! between a change here and its going up loses nothing, as the difference is still there.
//!
//! - **The flag** goes with its due date; its start is the day it was flagged (the server's, once
//!   it has one; today for a flag set here), its completed date when it is marked complete.
//! - **Categories** go as what was added and taken away here, which the host applies to the
//!   server's list as it is then (S17), so that a category added in Outlook meanwhile stays.
//! - **Moves out of the account** (into a local folder, another account) leave the server's copy
//!   until the message is fetched whole here **and** the mailbox file holding it is saved; only
//!   then is it removed there (S19).
//!
//! What comes back is taken in by server id: a change that was made becomes the last synced
//! state; one the server refused is put back to how the server has it and said once per run and
//! kind of change (S20); one the server no longer has the message for leaves here with it (S17).

use std::collections::BTreeMap;

use mailcraft_model::sync::{CategoriesUp, FlagUp, GraphMessage, MessageChange, MessageChangeDone, MessageOutcome, ServerMessage, same_category};
use mailcraft_model::{FlagStatus, Folder, FolderKind, Id, Mailbox, Message, Queued};
use mailcraft_time::{DateTime, Timestamp};

use crate::Session;

/// The changes of messages made in `account` that its server hasn't heard of: at most `limit`
/// spelled out, and how many there are.
pub fn message_changes(s: &Session, account: Id, limit: usize) -> (Vec<MessageChange>, usize) {
    let mb = &s.mailbox;
    let (mut changes, mut count) = (Vec::new(), 0usize);
    // (`None`: it can't go after all, which is looked at only for one that is spelled out.)
    let mut note = |change: &dyn Fn() -> Option<MessageChange>| {
        if changes.len() < limit {
            let Some(change) = change() else { return };
            changes.push(change);
        }
        count += 1;
    };
    let today = s.zone().instant(DateTime::at_midnight(s.today()));
    let now = s.now();
    // What waits in the Outbox is sent, not moved (R3.4.2).
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    for m in mb.messages.values().filter(|m| m.account == account && Some(m.folder) != outbox) {
        let Some(server) = m.server.as_ref().filter(|server| !server.gone && !server.id.is_empty()) else { continue };
        if let Some(update) = update_of(m, server, today, now) {
            note(&|| Some(update.clone()));
        }
        if let Some((to, to_folder)) = destination(mb, account, m.folder).filter(|(to, _)| *to != server.folder) {
            note(&|| Some(MessageChange::Move { message: m.id, id: server.id.clone(), to: to.clone(), to_folder }));
        }
    }
    for queued in mb.queued.iter().filter(|q| q.account() == account) {
        match queued {
            Queued::CopyMessage { id, to, copy, marker, .. } => {
                // Where the copy lies here now; a copy that is gone here isn't made.
                let Some(here) = mb.message(*copy) else { continue };
                let (to, to_folder) = match destination(mb, account, here.folder) {
                    Some(dest) => dest,
                    None if here.account == account => (to.clone(), None),
                    None => continue,
                };
                note(&|| Some(MessageChange::Copy { copy: *copy, id: id.clone(), to: to.clone(), to_folder, marker: marker.clone() }));
            }
            Queued::DeleteMessage { id, .. } => note(&|| Some(MessageChange::Delete { id: id.clone() })),
            // (Made by the run with the whole message: `graph_place`.)
            Queued::DeleteFolder { .. } | Queued::PlaceMessage { .. } => {}
        }
    }
    // Moved out of the account, whole, and saved so: the server's copy goes. Moved into another
    // mailbox of the same owner's (#70): once the copy is on that mailbox's server.
    let current = s.file_is_current();
    for m in mb.messages.values() {
        let Some(out) = m.moving_out.as_ref().filter(|out| out.account == account) else { continue };
        // Every part of one on this computer is read back only when it goes into the request: not
        // for a count, nor for what waits past the limit.
        if super::graph_place::safe_here(s, m, current, false) {
            note(&|| super::graph_place::safe_here(s, m, current, true).then(|| MessageChange::MovedOut { message: m.id, id: out.id.clone() }));
        }
    }
    (changes, count)
}

/// The server's id of the folder `folder` of `account`, and the folder itself when the server
/// is to make it in the same run (`to` empty then). `None` for a folder that isn't the account's
/// or isn't to be on the server.
pub(super) fn destination(mb: &Mailbox, account: Id, folder: Id) -> Option<(String, Option<Id>)> {
    let f: &Folder = mb.folder(folder).filter(|f| f.account == account)?;
    match &f.server {
        Some(server) => Some((server.id.clone(), None)),
        // A folder made here is made on the server by this run; the special ones are found there.
        // (Not one that is waiting for its place in the server's listing: it is never made, so a
        // move into it would wait for a make that does not come.)
        None if f.kind == FolderKind::Custom && !f.awaiting_listing => Some((String::new(), Some(f.id))),
        // An Exchange server account's Archive is a folder like another there (the server has no
        // folder of that kind): made when the owner puts mail in it (E8).
        None if made_on_server(mb, f) => Some((String::new(), Some(f.id))),
        None => None,
    }
}

/// Whether `f`, which has no server id, is a special folder of an Exchange server account that
/// the server doesn't have of its own accord: its Archive, once the account's folders are listed
/// (so that one the server has already is found, not made twice) and mail has been put in it. A
/// server that already has an `Archive` folder gives it its id by the listing, whose name is
/// matched (`graph_mail::adopt`); the Outbox is the queue here and never goes up.
pub(super) fn made_on_server(mb: &Mailbox, f: &Folder) -> bool {
    f.kind == FolderKind::Archive
        && !f.awaiting_listing
        && mb.account(f.account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Exchange)
        && mb.folder_of_kind(f.account, FolderKind::Inbox).and_then(|i| mb.folder(i)).is_some_and(|i| i.server.is_some())
        && mb.messages_in(f.id).next().is_some()
}

/// Whether two lists of categories hold the same names, whatever their order and case.
fn same_categories(a: &[String], b: &[String]) -> bool {
    let has = |list: &[String], name: &String| list.iter().any(|c| same_category(c, name));
    a.iter().all(|c| has(b, c)) && b.iter().all(|c| has(a, c))
}

/// The properties of `m` changed here since `server` said, as one change.
fn update_of(m: &Message, server: &ServerMessage, today: Timestamp, now: Timestamp) -> Option<MessageChange> {
    let read = (m.read != server.read).then_some(m.read);
    let flag_changed = m.flag != server.flag || (m.flag == FlagStatus::Flagged && m.flag_due != server.flag_due);
    let flag = flag_changed.then(|| match m.flag {
        FlagStatus::None => FlagUp { status: FlagStatus::None, ..Default::default() },
        FlagStatus::Flagged => {
            // The day it was flagged: the server's, when it had the flag already; today otherwise.
            let start = server.flag_start.filter(|_| server.flag != FlagStatus::None).unwrap_or(today);
            let start = m.flag_due.map_or(start, |due| start.min(due));
            FlagUp { status: FlagStatus::Flagged, start: Some(start), due: m.flag_due, completed: None }
        }
        FlagStatus::Complete => FlagUp {
            status: FlagStatus::Complete,
            completed: Some(server.flag_completed.filter(|_| server.flag == FlagStatus::Complete).unwrap_or(now)),
            ..Default::default()
        },
    });
    let categories =
        (!same_categories(&m.categories, &server.categories)).then(|| CategoriesUp { synced: server.categories.clone(), here: m.categories.clone() });
    // (A draft's importance goes up with the draft: R3.4.1.)
    let importance = (m.server_draft.is_none() && m.importance != server.importance).then_some(m.importance);
    if read.is_none() && flag.is_none() && categories.is_none() && importance.is_none() {
        return None;
    }
    Some(MessageChange::Update { message: m.id, id: server.id.clone(), read, flag, categories, importance })
}

/// A kind of change, for saying refusals once per run and kind (S20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Kind {
    Read(bool),
    Flag,
    Categories,
    Importance,
    Several,
    Move,
    Copy,
    Delete,
    MovedOut,
}

/// Why the server said no.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Why {
    Gone,
    Denied,
    Invalid,
}

impl Why {
    pub(super) fn of(outcome: &MessageOutcome) -> Option<Why> {
        match outcome {
            MessageOutcome::Done { .. } => None,
            MessageOutcome::Gone => Some(Why::Gone),
            MessageOutcome::Refused { status: 403 } => Some(Why::Denied),
            MessageOutcome::Refused { .. } => Some(Why::Invalid),
        }
    }

    fn words(self, many: bool) -> &'static str {
        match (self, many) {
            (Why::Gone, false) => "it no longer exists",
            (Why::Gone, true) => "they no longer exist",
            (Why::Denied, false) => "this account may not change it",
            (Why::Denied, true) => "this account may not change them",
            (Why::Invalid, _) => "it can't make that change",
        }
    }
}

/// The refusals of a run, by kind and reason: how many, the first message's subject, and the
/// folder they were to go to (when that was one and the same).
#[derive(Debug, Default)]
pub struct Refusals {
    said: BTreeMap<(Kind, Why), (usize, String, Option<String>)>,
}

impl Refusals {
    pub(super) fn note(&mut self, kind: Kind, why: Why, subject: &str, to: Option<String>) {
        // A delete for good of a message nothing is known of here is of "a message".
        let subject = if kind == Kind::Delete && subject.trim().is_empty() { String::new() } else { shown(subject) };
        let entry = self.said.entry((kind, why)).or_insert_with(|| (0, subject, to.clone()));
        entry.0 += 1;
        if entry.2 != to {
            entry.2 = None;
        }
    }

    /// One line for each kind and reason: "Couldn't move 'Invoice 42' to Archive: the server
    /// says it no longer exists."
    pub fn lines(&self) -> Vec<String> {
        self.said
            .iter()
            .map(|((kind, why), (n, subject, to))| {
                let many = *n > 1;
                let what = if many { format!("{n} messages") } else { format!("'{subject}'") };
                let to = to.as_ref().map(|to| format!(" to {to}")).unwrap_or_default();
                let doing = match kind {
                    Kind::Read(true) => format!("mark {what} as read"),
                    Kind::Read(false) => format!("mark {what} as unread"),
                    Kind::Flag => format!("change the flag of {what}"),
                    Kind::Categories => format!("change the categories of {what}"),
                    Kind::Importance => format!("change the importance of {what}"),
                    Kind::Several => format!("change {what}"),
                    Kind::Move => format!("move {what}{to}"),
                    Kind::Copy => format!("copy {what}{to}"),
                    Kind::Delete if subject.is_empty() && !many => "delete a message for good".to_string(),
                    Kind::Delete => format!("delete {what} for good"),
                    Kind::MovedOut => format!("remove {what} from the server after moving it here"),
                };
                match (kind, why) {
                    (Kind::Delete, Why::Denied) => format!("Couldn't {doing}: the server doesn't allow it."),
                    _ => format!("Couldn't {doing}: the server says {}.", why.words(many)),
                }
            })
            .collect()
    }
}

/// A subject as a line about it shows it.
fn shown(subject: &str) -> String {
    let subject: String = subject.chars().filter(|c| !c.is_control()).take(60).collect();
    if subject.trim().is_empty() { "(no subject)".to_string() } else { subject }
}

/// The name of the folder of `account` the server calls `id`, for a line about it.
pub(super) fn folder_name(mb: &Mailbox, account: Id, id: &str, local: Option<Id>) -> Option<String> {
    let folder = local.or_else(|| mb.folder_by_server(account, id))?;
    mb.folder(folder).map(|f| f.name.chars().filter(|c| !c.is_control()).take(60).collect())
}

/// The message of `account` called `id` on the server, when it is still `message` here.
fn mine<'m>(mb: &'m mut Mailbox, account: Id, message: Id, id: &str) -> Option<&'m mut Message> {
    let m = mb.message_mut(message).ok()?;
    (m.account == account && m.server.as_ref().is_some_and(|s| s.id == id)).then_some(m)
}

/// Take in what became of one change of a message. Whether anything changed here.
pub fn message_done(mb: &mut Mailbox, account: Id, done: &MessageChangeDone, refusals: &mut Refusals) -> bool {
    let why = Why::of(&done.outcome);
    match &done.change {
        MessageChange::Update { message, id, read, flag, categories, importance } => {
            let props = [read.is_some(), flag.is_some(), categories.is_some(), importance.is_some()];
            let kind = match props {
                _ if props.iter().filter(|p| **p).count() > 1 => Kind::Several,
                [true, ..] => Kind::Read(read.unwrap_or_default()),
                [_, true, ..] => Kind::Flag,
                [_, _, true, _] => Kind::Categories,
                _ => Kind::Importance,
            };
            let Some(m) = mine(mb, account, *message, id) else { return false };
            let Some(server) = m.server.clone() else { return false };
            let mut server = server;
            match &done.outcome {
                MessageOutcome::Done { categories: now, flag: went, .. } => {
                    if let Some(read) = read {
                        server.read = *read;
                    }
                    if let Some(flag) = went.as_ref().or(flag.as_ref()) {
                        server.flag = flag.status;
                        match flag.status {
                            FlagStatus::None => (server.flag_start, server.flag_due, server.flag_completed) = (None, None, None),
                            FlagStatus::Flagged => (server.flag_start, server.flag_due, server.flag_completed) = (flag.start, flag.due, None),
                            // Marked complete, the flag keeps its dates.
                            FlagStatus::Complete => server.flag_completed = flag.completed,
                        }
                    }
                    if let Some(up) = categories {
                        let now = now.clone().unwrap_or_else(|| up.here.clone());
                        // Changed again here meanwhile: that goes up next time.
                        if m.categories == up.here {
                            m.categories = now.clone();
                        }
                        server.categories = now;
                    }
                    if let Some(importance) = importance {
                        server.importance = *importance;
                    }
                }
                _ => {
                    // Put back to how the server has it, but for what was changed again here.
                    if read.is_some_and(|read| m.read == read) {
                        m.read = server.read;
                    }
                    if flag.as_ref().is_some_and(|f| m.flag == f.status) {
                        (m.flag, m.flag_due) = (server.flag, server.flag_due);
                    }
                    if categories.as_ref().is_some_and(|up| m.categories == up.here) {
                        m.categories = server.categories.clone();
                    }
                    if importance.is_some_and(|importance| m.importance == importance) {
                        m.importance = server.importance;
                    }
                    server.gone |= why == Some(Why::Gone);
                    let subject = m.subject.clone();
                    if let Some(why) = why {
                        refusals.note(kind, why, &subject, None);
                    }
                }
            }
            if let Ok(m) = mb.message_mut(*message) {
                m.server = Some(server);
            }
            true
        }
        MessageChange::Move { message, id, to, to_folder } => {
            let to_name = folder_name(mb, account, to, *to_folder);
            let Some(m) = mine(mb, account, *message, id) else { return false };
            let Some(server) = m.server.as_mut() else { return false };
            match &done.outcome {
                MessageOutcome::Done { folder, id: now, .. } => {
                    server.folder = if folder.is_empty() { to.clone() } else { folder.clone() };
                    // An Exchange server gives a moved message an id of its own in its new folder
                    // (a Microsoft account's ids stay: it says none).
                    if !now.is_empty() {
                        server.id = now.clone();
                    }
                }
                MessageOutcome::Gone => {
                    server.gone = true;
                    let subject = m.subject.clone();
                    refusals.note(Kind::Move, Why::Gone, &subject, to_name);
                }
                MessageOutcome::Refused { .. } => {
                    let was = server.folder.clone();
                    let subject = m.subject.clone();
                    if let Some(back) = mb.folder_by_server(account, &was)
                        && let Ok(m) = mb.message_mut(*message)
                    {
                        m.folder = back;
                    }
                    if let Some(why) = why {
                        refusals.note(Kind::Move, why, &subject, to_name);
                    }
                }
            }
            true
        }
        MessageChange::Copy { copy, id, to, to_folder, .. } => {
            let to_name = folder_name(mb, account, to, *to_folder);
            mb.queued.retain(|q| !matches!(q, Queued::CopyMessage { account: a, copy: c, .. } if *a == account && c == copy));
            let source =
                mb.messages.values().find(|m| m.account == account && m.server.as_ref().is_some_and(|s| s.id == *id)).and_then(|m| m.server.clone());
            match &done.outcome {
                MessageOutcome::Done { folder, id: made, .. } if !made.is_empty() => {
                    let folder = if folder.is_empty() { to.clone() } else { folder.clone() };
                    match mb.message_mut(*copy) {
                        Ok(m) if m.account == account => {
                            let base = source.unwrap_or_else(|| ServerMessage {
                                read: m.read,
                                flag: m.flag,
                                flag_due: m.flag_due,
                                categories: m.categories.clone(),
                                importance: m.importance,
                                ..Default::default()
                            });
                            m.server = Some(ServerMessage { id: made.clone(), folder, gone: false, ..base });
                        }
                        // Gone here meanwhile (or moved out of the account): the server's copy goes too.
                        _ => mb.queued.push(Queued::DeleteMessage { account, id: made.clone(), kept: None }),
                    }
                }
                MessageOutcome::Done { .. } => {}
                outcome => {
                    let subject = mb.message(*copy).map(|m| m.subject.clone()).unwrap_or_default();
                    // Refused: the server has no copy, and neither has this mailbox. A source that
                    // is gone leaves the copy here, which is all there is of it.
                    if matches!(outcome, MessageOutcome::Refused { .. }) {
                        let _ = mb.forget_message(*copy);
                    }
                    if let Some(why) = why {
                        refusals.note(Kind::Copy, why, &subject, to_name);
                    }
                }
            }
            true
        }
        MessageChange::Delete { id } => {
            let mut kept = None;
            mb.queued.retain(|q| match q {
                Queued::DeleteMessage { account: a, id: queued, kept: was } if *a == account && queued == id => {
                    kept = kept.take().or_else(|| was.clone());
                    false
                }
                _ => true,
            });
            if let (Some(why), MessageOutcome::Refused { .. }) = (why, &done.outcome) {
                let subject = put_back(mb, account, id, kept, done.there.as_deref());
                refusals.note(Kind::Delete, why, &subject, None);
            }
            true
        }
        MessageChange::MovedOut { message, id } => {
            let Ok(m) = mb.message_mut(*message) else { return false };
            if !m.moving_out.as_ref().is_some_and(|out| out.account == account && out.id == *id) {
                // Moved back into the account (or the move undone) while the server's copy was
                // being removed: it is gone there, and is put on the server again from here (#451).
                let back = m.account == account && m.server.as_ref().is_some_and(|s| s.id == *id);
                if !back || matches!(done.outcome, MessageOutcome::Refused { .. }) {
                    return false;
                }
                (m.server, m.uid, m.synced) = (None, 0, None);
                mb.place_again(*message);
                return true;
            }
            // Removed, gone already, or refused: either way the copy here is the one there is.
            m.moving_out = None;
            if let (Some(why), MessageOutcome::Refused { .. }) = (why, &done.outcome) {
                let subject = m.subject.clone();
                refusals.note(Kind::MovedOut, why, &subject, None);
            }
            true
        }
    }
}

/// A message deleted for good here whose delete the server refused, put back as the server has it
/// (S20): in its server folder, from what was kept of it here (all that was fetched), with the
/// server's state as it was read after the refusal or, failing that, the last synced one; from
/// what the server said of it alone when nothing was kept (it is fetched whole later in the run,
/// as new mail is). Kept but with nowhere on the server known (being moved out of the account, and
/// not to be read), it goes back where it was here. A folder the server has it in that isn't here
/// yet brings it down with its first listing. The subject, for saying so.
fn put_back(mb: &mut Mailbox, account: Id, id: &str, kept: Option<Box<Message>>, there: Option<&GraphMessage>) -> String {
    let subject = kept.as_ref().map(|m| m.subject.clone()).or_else(|| there.map(|t| t.subject.clone())).unwrap_or_default();
    // Back already (it came down again meanwhile): nothing to do.
    if mb.messages.values().any(|m| m.account == account && m.server.as_ref().is_some_and(|s| s.id == id)) {
        return subject;
    }
    let there = there.filter(|t| t.server.id == id);
    let server = there.map(|t| t.server.clone()).or_else(|| kept.as_ref().and_then(|m| m.server.clone()).filter(|s| s.id == id));
    let folder = server.as_ref().and_then(|s| mb.folder_by_server(account, &s.folder));
    match (kept, server, folder) {
        (Some(kept), Some(server), Some(folder)) => {
            let mut m = *kept;
            (m.folder, m.uid, m.synced, m.moving_out) = (folder, 0, None, None);
            (m.read, m.flag, m.flag_due, m.importance) = (server.read, server.flag, server.flag_due, server.importance);
            m.categories = server.categories.clone();
            // A size the server didn't say is the one kept (#300), as a delta's is.
            let size = if server.size > 0 { server.size } else { m.server.as_ref().map_or(0, |s| s.size) };
            m.server = Some(ServerMessage { id: id.to_string(), gone: false, size, ..server });
            let _ = mb.add_message(m);
        }
        (None, Some(server), Some(folder)) => {
            if let Some(there) = there {
                let _ = mb.add_message(super::graph_down::new_message(folder, &server.folder, there));
            }
        }
        (Some(kept), None, _) if mb.folder(kept.folder).is_some() => {
            let _ = mb.add_message(*kept);
        }
        _ => {}
    }
    subject
}

/// Messages the server said were gone before they could be fetched: one being moved out of the
/// account is kept as it is (its headers and preview), marked incomplete (S19).
pub fn missing(mb: &mut Mailbox, account: Id, ids: &[String]) -> usize {
    let mut marked = 0;
    for id in ids {
        let Some(found) = super::graph_down::moving_out(mb, account, id) else { continue };
        if let Ok(m) = mb.message_mut(found)
            && m.unfetched
        {
            (m.moving_out, m.incomplete) = (None, true);
            marked += 1;
        }
    }
    marked
}

#[cfg(test)]
#[path = "graph_up_tests.rs"]
mod tests;
