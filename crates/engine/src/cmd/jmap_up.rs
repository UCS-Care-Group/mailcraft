// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Changes of messages up to a JMAP server (M6), as they go up through Microsoft Graph
//! ([`super::graph_up`]): what goes is the difference between each message of the account and
//! what its server last said of it ([`ServerMessage`]), so that a crash between a change here and
//! its going up loses nothing. Only a deletion for good, which no comparing finds, waits in the
//! mailbox's queue ([`Queued::DeleteMessage`]).
//!
//! - **Marks** go as the server's keywords: read (`$seen`), flagged (`$flagged`), answered
//!   (`$answered`) and forwarded (`$forwarded`), only those that differ, as one change of the
//!   message. A follow-up marked complete here is not flagged there, and its due date stays here:
//!   the server keeps `$flagged` and nothing else, so only that bit is compared, here and when a
//!   run reads the marks ([`super::graph_down`] takes the server's flag when the bit is as the
//!   server last said here and another there).
//! - **A move** goes when the message lies in another of the server's mailboxes than the server
//!   last said. Delete is a move into the server's Deleted Items, as for a Microsoft account. A
//!   folder the server hasn't got takes none of its mail ([`mailcraft_model::Mailbox::stays_out_of`]).
//! - **A deletion for good** goes as one there.
//!
//! A run takes [`MAX_JMAP_CHANGES`] at the most; the rest is worked out again for the next.
//!
//! What comes back is a host's and is held to sizes first ([`bounded`]). A change that was made
//! becomes what the server last said (with the message's new id, when a move gave it one); one for
//! a message the server no longer has marks it gone, for the run's windows to settle; one the
//! server refused is put back to how the server has it, but for what was changed again here, and
//! said once per run, kind of change and reason. Put back, it is no difference any more: it isn't
//! sent again unless the owner makes it again. A change with no answer is still a difference, and
//! goes with the next run.

use std::collections::{BTreeMap, HashMap};

use mailcraft_model::sync::{JmapChange, JmapChangeDone, JmapMailResult, JmapOutcome, ServerMessage};
use mailcraft_model::{FlagStatus, FolderKind, Id, Mailbox, Message, Queued};

use super::graph_up::{self, Kind, Refusals, Why};
use super::jmap_down::{MAX_LINE, is_id, line};

/// Most changes of messages one run takes up, and most answers taken in of one piece: what a
/// server takes in one `Email/set` at the least (RFC 8620 §2, `maxObjectsInSet`).
pub const MAX_JMAP_CHANGES: usize = 500;

/// The marks of `m` that differ from what `server` last said, as one change; none when none does.
fn keywords_of(m: &Message, server: &ServerMessage) -> Option<JmapChange> {
    let differs = |here: bool, there: bool| (here != there).then_some(here);
    let read = differs(m.read, server.read);
    let flagged = differs(m.flag == FlagStatus::Flagged, server.flag == FlagStatus::Flagged);
    let answered = differs(m.answered, server.answered);
    let forwarded = differs(m.forwarded, server.forwarded);
    if read.is_none() && flagged.is_none() && answered.is_none() && forwarded.is_none() {
        return None;
    }
    Some(JmapChange::Keywords { message: m.id, id: server.id.clone(), read, flagged, answered, forwarded })
}

/// The server's id of the mailbox each folder of `account` is, for those the server has: the
/// folders its mail can be moved into, by the one test of that ([`Mailbox::jmap_mailbox`]).
fn mailboxes_of(mb: &Mailbox, account: Id) -> HashMap<Id, &str> {
    mb.folders.iter().filter_map(|f| Mailbox::jmap_mailbox_of(f, account).map(|mailbox| (f.id, mailbox))).collect()
}

/// The changes of messages made in `account` that its server hasn't heard of: at most `limit`
/// spelled out, and how many there are. What lies in the Outbox is no change of a message.
pub fn changes(mb: &Mailbox, account: Id, limit: usize) -> (Vec<JmapChange>, usize) {
    let (mut changes, mut count) = (Vec::new(), 0usize);
    let mut note = |change: &dyn Fn() -> JmapChange| {
        if changes.len() < limit {
            changes.push(change());
        }
        count = count.saturating_add(1);
    };
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    let mailboxes = mailboxes_of(mb, account);
    for m in mb.messages.values().filter(|m| m.account == account && Some(m.folder) != outbox) {
        let Some(server) = m.server.as_ref().filter(|server| !server.gone && is_id(&server.id)) else { continue };
        if let Some(keywords) = keywords_of(m, server) {
            note(&|| keywords.clone());
        }
        if let Some(to) = mailboxes.get(&m.folder).copied().filter(|to| *to != server.folder && is_id(&server.folder)) {
            note(&|| JmapChange::Move { message: m.id, id: server.id.clone(), from: server.folder.clone(), to: to.to_string() });
        }
    }
    for queued in mb.queued.iter().filter(|q| q.account() == account) {
        if let Queued::DeleteMessage { id, .. } = queued
            && is_id(id)
        {
            note(&|| JmapChange::Destroy { id: id.clone() });
        }
    }
    (changes, count)
}

/// How many changes made in `account` its server hasn't heard of yet ([`super::account::unpushed`]
/// for an account kept in step over JMAP).
pub fn unpushed(mb: &Mailbox, account: Id) -> usize {
    changes(mb, account, 0).1
}

/// Whether what became of a change can be taken in at all (its ids are ids), with its outcome held
/// to what a server says: a new id that is none is no new id, and a reason is one line.
fn bounded_change(done: &mut JmapChangeDone) -> bool {
    let named = match &done.change {
        JmapChange::Keywords { id, .. } | JmapChange::Destroy { id } => is_id(id),
        JmapChange::Move { id, from, to, .. } => is_id(id) && is_id(from) && is_id(to),
    };
    match &mut done.outcome {
        JmapOutcome::Done { id } if !id.is_empty() && !is_id(id) => id.clear(),
        JmapOutcome::Refused { why } => *why = line(why, MAX_LINE),
        JmapOutcome::Done { .. } | JmapOutcome::Gone => {}
    }
    named
}

/// What a piece says became of the changes, held to sizes: as many answers as a run asks about,
/// ids that are ids, reasons of one line. What became of mail sent is taken out: nothing is sent
/// from such an account yet, so it answers nothing that was asked.
pub(crate) fn bounded(result: &mut JmapMailResult) {
    result.changes.truncate(MAX_JMAP_CHANGES);
    result.changes.retain_mut(bounded_change);
    result.sent.clear();
}

/// What the server wouldn't do in a run, for saying it once per kind of change and reason: what it
/// no longer has the message for in the words a Microsoft account's refusals are said in, and
/// what it refused with the reason it gave.
#[derive(Debug, Default)]
pub(crate) struct Told {
    usual: Refusals,
    /// By kind and the server's reason: how many, the first message's subject, and the folder they
    /// were to go to (when that was one and the same).
    said: BTreeMap<(Kind, String), (usize, String, Option<String>)>,
}

impl Told {
    fn gone(&mut self, kind: Kind, subject: &str, to: Option<String>) {
        self.usual.note(kind, Why::Gone, subject, to);
    }

    fn refused(&mut self, kind: Kind, why: &str, subject: &str, to: Option<String>) {
        let why = why.trim().trim_end_matches('.').trim_end();
        if why.is_empty() {
            return self.usual.note(kind, Why::Invalid, subject, to);
        }
        // A delete for good of a message nothing is known of here is of "a message".
        let subject = if kind == Kind::Delete && subject.trim().is_empty() { String::new() } else { graph_up::shown(subject) };
        let entry = self.said.entry((kind, why.to_string())).or_insert_with(|| (0, subject, to.clone()));
        entry.0 = entry.0.saturating_add(1);
        if entry.2 != to {
            entry.2 = None;
        }
    }

    /// One line for each kind and reason: "Couldn't move 'Invoice 42' to Archive: the mailbox is
    /// read-only."
    pub(crate) fn lines(&self) -> Vec<String> {
        let said = self.said.iter().map(|((kind, why), (n, subject, to))| {
            let doing = graph_up::doing(*kind, *n, subject, to.as_deref());
            format!("Couldn't {doing}: {why}.")
        });
        self.usual.lines().into_iter().chain(said).collect()
    }
}

/// The kind of change a change of marks is, for a line about it.
fn kind_of(read: Option<bool>, flagged: Option<bool>, answered: Option<bool>, forwarded: Option<bool>) -> Kind {
    match (read, flagged, answered.or(forwarded)) {
        (Some(read), None, None) => Kind::Read(read),
        (None, Some(_), None) => Kind::Flag,
        _ => Kind::Several,
    }
}

/// What became of a change, in a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Became {
    /// Nothing here was changed by the answer (it was for a message that is another now).
    Nothing,
    /// The server made it.
    Made,
    /// The server didn't: what is here was brought in step with that.
    PutBack,
    /// No message here was changed, but what waits to go to the server was (a deletion that
    /// waits is now of the id the server gave the message): the mailbox is to be saved.
    Waits,
}

/// A move was made for a message that was deleted for good here while the move was on its way:
/// the deletion that waits for the server is of the message where, and under the id, the server
/// has it now. Without this a server that gives a moved message an id of its own would be asked to
/// delete the old id, answer that it has none such, and keep the message.
/// Whether a deletion that waits was changed by it.
fn moved_while_deleted(mb: &mut Mailbox, account: Id, id: &str, to: &str, outcome: &JmapOutcome) -> bool {
    let JmapOutcome::Done { id: now } = outcome else { return false };
    let mut changed = false;
    for queued in &mut mb.queued {
        let Queued::DeleteMessage { account: a, id: waiting, kept } = queued else { continue };
        if *a != account || waiting != id {
            continue;
        }
        changed = true;
        if let Some(server) = kept.as_mut().and_then(|kept| kept.server.as_mut()) {
            server.folder = to.to_string();
            if !now.is_empty() {
                server.id = now.clone();
            }
        }
        if !now.is_empty() {
            *waiting = now.clone();
        }
    }
    changed
}

/// Take in what became of one change of a message of `account`.
pub(crate) fn change_done(mb: &mut Mailbox, account: Id, done: &JmapChangeDone, told: &mut Told) -> Became {
    match &done.change {
        JmapChange::Keywords { message, id, read, flagged, answered, forwarded } => {
            let kind = kind_of(*read, *flagged, *answered, *forwarded);
            let Some(m) = graph_up::mine(mb, account, *message, id) else { return Became::Nothing };
            let Some(mut server) = m.server.clone() else { return Became::Nothing };
            let became = match &done.outcome {
                JmapOutcome::Done { .. } => {
                    server.read = read.unwrap_or(server.read);
                    server.answered = answered.unwrap_or(server.answered);
                    server.forwarded = forwarded.unwrap_or(server.forwarded);
                    match flagged {
                        Some(true) => server.flag = FlagStatus::Flagged,
                        Some(false) => {
                            server.flag = FlagStatus::None;
                            (server.flag_start, server.flag_due, server.flag_completed) = (None, None, None);
                        }
                        None => {}
                    }
                    Became::Made
                }
                outcome => {
                    // Put back to how the server has it, but for what was changed again here.
                    if read.is_some_and(|read| m.read == read) {
                        m.read = server.read;
                    }
                    // (Flagged again after a "complete" the server wouldn't take, a follow-up keeps
                    // the date it was due, which only this computer knows; not flagged, it has none.)
                    if flagged.is_some_and(|flagged| (m.flag == FlagStatus::Flagged) == flagged) {
                        m.flag = server.flag;
                        if m.flag != FlagStatus::Flagged {
                            m.flag_due = None;
                        }
                    }
                    if answered.is_some_and(|answered| m.answered == answered) {
                        m.answered = server.answered;
                    }
                    if forwarded.is_some_and(|forwarded| m.forwarded == forwarded) {
                        m.forwarded = server.forwarded;
                    }
                    match outcome {
                        JmapOutcome::Refused { why } => told.refused(kind, why, &m.subject, None),
                        _ => {
                            server.gone = true;
                            told.gone(kind, &m.subject, None);
                        }
                    }
                    Became::PutBack
                }
            };
            m.server = Some(server);
            became
        }
        JmapChange::Move { message, id, to, .. } => {
            let to_name = graph_up::folder_name(mb, account, to, None);
            let Some(m) = graph_up::mine(mb, account, *message, id) else {
                let waits = moved_while_deleted(mb, account, id, to, &done.outcome);
                return if waits { Became::Waits } else { Became::Nothing };
            };
            let (subject, lies_in) = (m.subject.clone(), m.folder);
            let Some(server) = m.server.as_mut() else { return Became::Nothing };
            match &done.outcome {
                JmapOutcome::Done { id: now } => {
                    server.folder = to.clone();
                    // A server may give a moved message an id of its own in its new mailbox.
                    if !now.is_empty() {
                        server.id = now.clone();
                    }
                    Became::Made
                }
                JmapOutcome::Gone => {
                    server.gone = true;
                    told.gone(Kind::Move, &subject, to_name);
                    Became::PutBack
                }
                JmapOutcome::Refused { why } => {
                    let was = server.folder.clone();
                    // Back where the server has it, unless it was moved on here meanwhile (that
                    // is another move, which goes up by itself). With nowhere to go back to (the
                    // mailbox it was in isn't here), it stays, and the server's word is taken to
                    // be where it lies, so that the same move isn't asked for at every run: the
                    // next window that lists it says where it is.
                    let there = mb.folder_by_server(account, to);
                    match mb.folder_by_server(account, &was) {
                        Some(back) if there == Some(lies_in) => {
                            if let Ok(m) = mb.message_mut(*message) {
                                m.folder = back;
                            }
                        }
                        Some(_) => {}
                        None => {
                            if let Some(server) = mb.message_mut(*message).ok().and_then(|m| m.server.as_mut()) {
                                server.folder = to.clone();
                            }
                        }
                    }
                    told.refused(Kind::Move, why, &subject, to_name);
                    Became::PutBack
                }
            }
        }
        JmapChange::Destroy { id } => {
            let mut kept = None;
            let before = mb.queued.len();
            mb.queued.retain(|q| match q {
                Queued::DeleteMessage { account: a, id: queued, kept: was } if *a == account && queued == id => {
                    kept = kept.take().or_else(|| was.clone());
                    false
                }
                _ => true,
            });
            // (An answer for a deletion nobody asked for here changes nothing.)
            if mb.queued.len() == before {
                return Became::Nothing;
            }
            match &done.outcome {
                JmapOutcome::Done { .. } | JmapOutcome::Gone => Became::Made,
                // The server keeps it: it is put back from what was kept of it here, in the
                // mailbox the server has it in; one nothing was kept of comes down with the next
                // window that lists it.
                JmapOutcome::Refused { why } => {
                    let subject = graph_up::put_back(mb, account, id, kept, None);
                    told.refused(Kind::Delete, why, &subject, None);
                    Became::PutBack
                }
            }
        }
    }
}
