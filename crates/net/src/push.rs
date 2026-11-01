// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pushing the changes made here to the IMAP server: flags, moves and deletions
//! ([`SyncRequest::ops`]), one source mailbox at a time, and then the messages moved or copied
//! into the account from elsewhere ([`SyncRequest::places`], #451), before anything is
//! downloaded.
//!
//! Mail is never lost to a push. A message leaves a mailbox only by UID MOVE, or, on a server
//! without it, once its copy is in the other mailbox. A message is removed for good only when
//! the owner deleted it, and then by its UID (UID EXPUNGE, RFC 4315); a server that can't do
//! that keeps it marked `\Deleted`, for the mailbox-wide EXPUNGE would take what other programs
//! marked too. A change is only made in a mailbox whose `UIDVALIDITY` is the one the UIDs were
//! learned under, and when a step fails the rest of that mailbox's changes wait for the next run.
//!
//! Gmail (which says so with `X-GM-EXT-1`) keeps mail under labels, which IMAP shows as
//! mailboxes, and every message that is not in the trash in its mailbox of all mail (`\All`,
//! `[Gmail]/All Mail`). Archiving there takes the Inbox label away: a message archived here is
//! moved to all mail when the server has no mailbox for archived mail, and no label is made for
//! it. Expunging a message from a label only takes that label away, so a message deleted for good
//! here is moved to the trash first and expunged there.

use std::collections::{BTreeMap, HashMap, HashSet};

use mailcraft_mime::{PLACE_HEADER, WriteOptions};
use mailcraft_model::sync::{
    Created, Deleted, FlagsPushed, ImapPlace, MAX_PLACES, Moved, Placed, RemoteFlags, RemoteOp, SyncRequest, SyncResult, SyncStage, SyncStep,
};
use mailcraft_model::{FolderKind, Id};
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::{Copied, Imap, Mailbox, decode_utf7, encode_utf7, searchable, uid_sets};

use crate::sync::{Run, label, shown, subject_of};
use crate::transport::Stream;
use crate::{NetError, kinds};

/// Most changes pushed in one run; the rest waits for the next one.
const MAX_OPS: usize = 10_000;
/// Deepest a mailbox is made below one the server has.
const MAX_LEVELS: usize = 32;
/// Most messages with a placed message's `Message-ID` that are looked at to find its copy.
const MAX_LOOKED_AT: usize = 3;
/// Longest marker a copy carries (the engine's are far shorter).
const MAX_MARKER: usize = 200;

/// Messages that moves of this run put into mailboxes, by the mailbox's name here: the
/// `UIDVALIDITY` the server gave with their new UIDs, and those UIDs. The download takes them
/// for known instead of fetching them again.
pub(crate) type Arrived = HashMap<String, Vec<(u32, u32)>>;

/// Push the request's changes. What went through is noted in the run's answer (`flagged`,
/// `moved`, `deleted`, `created`), what didn't in its `errors`, and handed over after each
/// mailbox: the sooner the mailbox here knows a change was made, the less there is for a run
/// that ends early to find done. `None` when the connection was lost.
///
/// A mailbox's changes are made together: a run that is called off stops between two mailboxes,
/// never between a copy and the removal of its original.
pub(crate) fn push(run: &mut Run, imap: &mut Imap<Stream<'_>>, mailboxes: &[Mailbox]) -> Option<Arrived> {
    let request = run.request;
    let ops: Vec<&RemoteOp> = request.ops.iter().take(MAX_OPS).collect();
    let mut push = Push {
        request,
        mailboxes,
        can_move: false,
        uidplus: false,
        gmail: false,
        destinations: HashMap::new(),
        arrived: Arrived::new(),
        searching: None,
    };
    if ops.iter().any(|op| !matches!(op, RemoteOp::Flags { .. })) || !request.places.is_empty() {
        // Asked now: what a server can do is often only told once it knows who is asking.
        match imap.capabilities() {
            Ok(capabilities) => {
                push.can_move = capabilities.iter().any(|c| c == "MOVE");
                push.uidplus = capabilities.iter().any(|c| c == "UIDPLUS");
                push.gmail = capabilities.iter().any(|c| c.eq_ignore_ascii_case("X-GM-EXT-1"));
            }
            // A server that won't say is taken for one that can do neither.
            Err(ProtoError::Rejected(_)) => {}
            Err(e) => {
                run.out.errors.push(format!("Couldn't bring the server up to date: {e}"));
                return None;
            }
        }
    }
    // One mailbox after the other, in the order they are first spoken of.
    let mut sources: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for op in &ops {
        if seen.insert(op.mailbox()) {
            sources.push(op.mailbox());
        }
    }
    for remote in sources {
        if run.cancelled() {
            break;
        }
        let here: Vec<&RemoteOp> = ops.iter().copied().filter(|op| op.mailbox() == remote).collect();
        let alive = push.mailbox(imap, remote, &here, &mut run.out);
        if run.out.has_news() || !run.out.errors.is_empty() {
            run.hand_over_in(imap, SyncStep::new(SyncStage::Updating, &label(remote), 0, 0));
        }
        if !alive {
            return None;
        }
    }
    if !request.places.is_empty() && !run.cancelled() {
        let alive = push.places(imap, &mut run.out);
        if run.out.has_news() || !run.out.errors.is_empty() {
            run.hand_over_in(imap, SyncStep::new(SyncStage::Updating, "", 0, 0));
        }
        if !alive {
            return None;
        }
    }
    Some(push.arrived)
}

/// Where the messages moved to a local folder go on the server.
#[derive(Clone)]
struct Destination {
    /// The mailbox's name as the server writes it.
    raw: String,
    /// Its name here ([`kinds::remote_name`]).
    remote: String,
}

struct Push<'a> {
    request: &'a SyncRequest,
    /// The server's mailboxes, as listed before the push.
    mailboxes: &'a [Mailbox],
    /// The server knows UID MOVE.
    can_move: bool,
    /// The server knows UID EXPUNGE.
    uidplus: bool,
    /// The server is Gmail's, or works as it does (`X-GM-EXT-1`): see the module's notes.
    gmail: bool,
    /// The destination worked out for each local folder; `None` for one that can't be had (the
    /// reason was reported once).
    destinations: HashMap<Id, Option<Destination>>,
    arrived: Arrived,
    /// The destination open to be looked in for copies put there before ([`Push::copies`]): its
    /// name as the server writes it, and the `UIDVALIDITY` it was opened with.
    searching: Option<(String, u32)>,
}

/// Note a problem that ends the work on a mailbox; answers whether the connection is still good.
fn stop(result: &mut SyncResult, line: String, error: &NetError) -> bool {
    result.errors.push(line);
    !error.is_fatal()
}

/// `Err` when the connection `e` came from can't be used any more; a refusal is `Ok`.
fn fatal(e: impl Into<NetError>) -> Result<(), NetError> {
    let e = e.into();
    if e.is_fatal() { Err(e) } else { Ok(()) }
}

/// Whether `marker` can be written into a copy's header and asked for on a server.
fn marked(marker: &str) -> bool {
    !marker.is_empty() && marker.len() <= MAX_MARKER && marker.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn has(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
}

impl<'a> Push<'a> {
    /// Make the changes `ops` ask for in the mailbox known here as `remote`. Returns whether the
    /// connection is still good for the next mailbox.
    fn mailbox(&mut self, imap: &mut Imap<Stream<'_>>, remote: &str, ops: &[&RemoteOp], result: &mut SyncResult) -> bool {
        let name = shown(remote);
        let Some(mailbox) = self.mailboxes.iter().find(|m| kinds::selectable(m) && kinds::remote_name(m) == remote) else {
            // Not in the server's list. It may be gone for good, or the list may be short this
            // once; taking it for gone would call its changes done that were never made. They
            // wait, and the owner is told (removing the folder here ends the wait).
            result.errors.push(format!("Couldn't bring \"{name}\" up to date: the folder is not on the server"));
            return true;
        };
        // UIDs are good for one life of a mailbox only. Without knowing which life the request's
        // UIDs are from, nothing is touched.
        let learned_under = self.request.folders.iter().find(|f| f.remote == remote).map_or(0, |f| f.uid_validity);
        if learned_under == 0 {
            return true;
        }
        let selected = match imap.select(&mailbox.raw, false) {
            Ok(selected) => selected,
            Err(e) => return stop(result, format!("Couldn't open \"{name}\" to bring it up to date: {e}"), &e.into()),
        };
        if selected.uid_validity != learned_under {
            // The mailbox was made anew: the UIDs name other messages now, or none. The download
            // tells, and what was to be pushed is dropped where the request came from.
            return true;
        }
        match self.changes(imap, remote, &name, selected.exists, ops, result) {
            Ok(()) => true,
            Err(alive) => alive,
        }
    }

    /// Flags first, then moves, then deletions. `Err` ends the work on this mailbox, with
    /// whether the connection is still good.
    fn changes(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        remote: &str,
        name: &str,
        exists: u32,
        ops: &[&RemoteOp],
        result: &mut SyncResult,
    ) -> Result<(), bool> {
        self.flags(imap, remote, name, ops, result)?;
        let leaving: Vec<u32> = ops
            .iter()
            .filter_map(|op| match op {
                RemoteOp::Move { uid, .. } | RemoteOp::Delete { uid, .. } => Some(*uid),
                RemoteOp::Flags { .. } => None,
            })
            .collect();
        if leaving.is_empty() {
            return Ok(());
        }
        // Which of them the server still has. One that is gone, or marked as deleted (on its way
        // out, and shown by no mail program), leaves nothing to do.
        let mut present: HashSet<u32> = HashSet::new();
        for (set, _) in uid_sets(&leaving) {
            match imap.uid_fetch_flags(&set) {
                Ok(listed) => present.extend(listed.iter().filter(|m| !has(&m.flags, "\\Deleted")).map(|m| m.uid)),
                // Some servers refuse to look into an empty mailbox rather than answer with nothing.
                Err(ProtoError::Rejected(_)) if exists == 0 => {}
                Err(e) => return Err(stop(result, format!("Couldn't read \"{name}\": {e}"), &e.into())),
            }
        }
        self.moves(imap, name, ops, &present, result)?;
        self.deletes(imap, remote, name, ops, &present, result)
    }

    /// One UID STORE per flag and direction, for all the messages that need it.
    fn flags(&mut self, imap: &mut Imap<Stream<'_>>, remote: &str, name: &str, ops: &[&RemoteOp], result: &mut SyncResult) -> Result<(), bool> {
        let mut changes: Vec<(&'static str, bool, Vec<u32>)> = Vec::new();
        for op in ops {
            let RemoteOp::Flags { uid, read, flagged, answered, .. } = op else { continue };
            for (flag, wanted) in [("\\Seen", read), ("\\Flagged", flagged), ("\\Answered", answered)] {
                let Some(on) = *wanted else { continue };
                match changes.iter_mut().find(|(f, o, _)| *f == flag && *o == on) {
                    Some((_, _, uids)) => uids.push(*uid),
                    None => changes.push((flag, on, vec![*uid])),
                }
            }
        }
        // What went through, message by message: a later step may fail.
        let mut pushed: BTreeMap<u32, FlagsPushed> = BTreeMap::new();
        let mut outcome = Ok(());
        'changes: for (flag, on, uids) in &changes {
            for (set, uids) in uid_sets(uids) {
                if let Err(e) = imap.uid_store_set(&set, *on, &[*flag]) {
                    outcome = Err(stop(result, format!("Couldn't change flags in \"{name}\": {e}"), &e.into()));
                    break 'changes;
                }
                for uid in uids {
                    let entry = pushed.entry(uid).or_insert_with(|| FlagsPushed { folder: remote.to_string(), uid, ..Default::default() });
                    match *flag {
                        "\\Seen" => entry.read = Some(*on),
                        "\\Flagged" => entry.flagged = Some(*on),
                        _ => entry.answered = Some(*on),
                    }
                }
            }
        }
        result.flagged.extend(pushed.into_values());
        outcome
    }

    /// Move the messages that are still there, one command per destination.
    fn moves(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        name: &str,
        ops: &[&RemoteOp],
        present: &HashSet<u32>,
        result: &mut SyncResult,
    ) -> Result<(), bool> {
        // By destination, in the order they are first spoken of.
        let mut batches: Vec<(Id, Vec<&RemoteOp>)> = Vec::new();
        for op in ops {
            let RemoteOp::Move { message, from, uid, to_folder, .. } = op else { continue };
            if !present.contains(uid) {
                self.adopt(op, result);
                result.moved.push(Moved { message: *message, from: from.clone(), uid: *uid, to_folder: *to_folder, ..Default::default() });
                continue;
            }
            match batches.iter_mut().find(|(folder, _)| folder == to_folder) {
                Some((_, batch)) => batch.push(op),
                None => batches.push((*to_folder, vec![op])),
            }
        }
        for (_, batch) in batches {
            let Some(first) = batch.first() else { continue };
            // A destination that can't be had: its messages stay where they are, and wait.
            let Some(destination) = self.destination(imap, first, result)? else { continue };
            let mut by_uid: HashMap<u32, &RemoteOp> = HashMap::new();
            for op in &batch {
                if let RemoteOp::Move { uid, .. } = op {
                    by_uid.insert(*uid, op);
                }
            }
            let uids: Vec<u32> = by_uid.keys().copied().collect();
            for (set, uids) in uid_sets(&uids) {
                let went = if self.can_move { imap.uid_move(&set, &destination.raw) } else { imap.uid_copy(&set, &destination.raw) };
                let copied = match went {
                    Ok(copied) => copied.unwrap_or_default(),
                    Err(e) => {
                        let line = format!("Couldn't move messages from \"{name}\" to \"{}\": {e}", shown(&destination.remote));
                        return Err(stop(result, line, &e.into()));
                    }
                };
                // They are in their new mailbox, whatever comes next.
                self.note_moved(&by_uid, &uids, &copied, &destination, result);
                if self.can_move {
                    continue;
                }
                // Copied: now the originals go, as far as this server lets a single message go.
                match self.remove(imap, &set) {
                    Ok(()) => {}
                    // A copy that can't be taken back is no reason to copy again next time: the
                    // move is done, and the original stays for the owner to see to.
                    Err(Removal::Unmarked(e)) => {
                        let line =
                            format!("Messages moved from \"{name}\" to \"{}\" are still in \"{name}\" as well: {e}", shown(&destination.remote));
                        if !stop(result, line, &e.into()) {
                            return Err(false);
                        }
                    }
                    // Marked as deleted and still there: only a lost connection is worth a word.
                    Err(Removal::Marked(e)) => {
                        let e = NetError::from(e);
                        if e.is_fatal() {
                            return Err(stop(result, format!("Couldn't bring \"{name}\" up to date: {e}"), &e));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Note the moves of `uids` as finished, each with its UID in `destination` when `copied` says.
    fn note_moved(&mut self, by_uid: &HashMap<u32, &RemoteOp>, uids: &[u32], copied: &Copied, destination: &Destination, result: &mut SyncResult) {
        let new_uids: HashMap<u32, u32> = copied.uids.iter().copied().collect();
        for uid in uids {
            let Some(RemoteOp::Move { message, from, to_folder, .. }) = by_uid.get(uid) else { continue };
            let new_uid = new_uids.get(uid).copied().unwrap_or(0);
            let uid_validity = if new_uid == 0 { 0 } else { copied.uid_validity };
            result.moved.push(Moved { message: *message, from: from.clone(), uid: *uid, to_folder: *to_folder, new_uid, uid_validity });
            if new_uid != 0 {
                self.arrived.entry(destination.remote.clone()).or_default().push((uid_validity, new_uid));
            }
        }
    }

    /// Delete for good the messages that are still there.
    fn deletes(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        remote: &str,
        name: &str,
        ops: &[&RemoteOp],
        present: &HashSet<u32>,
        result: &mut SyncResult,
    ) -> Result<(), bool> {
        let mut uids = Vec::new();
        for op in ops {
            let RemoteOp::Delete { uid, .. } = op else { continue };
            if present.contains(uid) {
                uids.push(*uid);
            } else {
                result.deleted.push(Deleted { from: remote.to_string(), uid: *uid });
            }
        }
        if let Some(trash) = self.gmail_trash().filter(|trash| kinds::remote_name(trash) != remote && !uids.is_empty()) {
            return self.delete_through(imap, remote, name, &uids, trash, result);
        }
        for (set, uids) in uid_sets(&uids) {
            let removed = self.remove(imap, &set);
            // Marked as deleted is as good as gone: no mail program shows it, and the server
            // drops it with its next clean-up.
            if !matches!(removed, Err(Removal::Unmarked(_))) {
                result.deleted.extend(uids.iter().map(|uid| Deleted { from: remote.to_string(), uid: *uid }));
            }
            match removed {
                Ok(()) => {}
                Err(Removal::Unmarked(e)) => return Err(stop(result, format!("Couldn't delete messages from \"{name}\": {e}"), &e.into())),
                // Marked and still there: only a lost connection is worth a word.
                Err(Removal::Marked(e)) => {
                    let e = NetError::from(e);
                    if e.is_fatal() {
                        return Err(stop(result, format!("Couldn't bring \"{name}\" up to date: {e}"), &e));
                    }
                }
            }
        }
        Ok(())
    }

    /// Gmail's trash, where a message deleted for good is put before it is expunged: at Gmail an
    /// expunge from any other mailbox only takes a label away. `None` at another server, and at
    /// one that can't move a message in one step (copying it there would leave the original).
    fn gmail_trash(&self) -> Option<&'a Mailbox> {
        let mailboxes = self.mailboxes;
        let has = |flag: &str| mailboxes.iter().find(|m| kinds::selectable(m) && m.has_flag(flag));
        (self.gmail && self.can_move).then(|| has("\\All").and(has("\\Trash"))).flatten()
    }

    /// Delete for good, at Gmail, the messages `uids` of the mailbox known here as `remote`: move
    /// them to the `trash`, then remove them there by the UIDs the server gave them. A message
    /// that is in the trash is as good as gone (no mail program shows it there as anything but
    /// deleted, and Gmail empties it by itself), so it is noted as deleted once it is there; one
    /// that is left there because the server didn't say its new UID, or wouldn't remove it, comes
    /// down into Deleted Items with the next run, and is not lost.
    fn delete_through(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        remote: &str,
        name: &str,
        uids: &[u32],
        trash: &Mailbox,
        result: &mut SyncResult,
    ) -> Result<(), bool> {
        let mut there: Vec<(u32, u32)> = Vec::new();
        let mut outcome = Ok(());
        for (set, uids) in uid_sets(uids) {
            match imap.uid_move(&set, &trash.raw) {
                Ok(copied) => {
                    result.deleted.extend(uids.iter().map(|uid| Deleted { from: remote.to_string(), uid: *uid }));
                    let copied = copied.unwrap_or_default();
                    there.extend(copied.uids.iter().map(|(_, new_uid)| (copied.uid_validity, *new_uid)).filter(|(_, uid)| *uid != 0));
                }
                Err(e) => {
                    outcome = Err(stop(result, format!("Couldn't delete messages from \"{name}\": {e}"), &e.into()));
                    break;
                }
            }
        }
        if there.is_empty() {
            return outcome;
        }
        let trash_name = shown(&kinds::remote_name(trash));
        let selected = match imap.select(&trash.raw, false) {
            Ok(selected) => selected,
            Err(e) => return Err(stop(result, format!("Couldn't open \"{trash_name}\" to delete messages for good: {e}"), &e.into())),
        };
        // UIDs the server gave under another life of the mailbox name other messages now.
        let new_uids: Vec<u32> = there.iter().filter(|(validity, _)| *validity == selected.uid_validity).map(|(_, uid)| *uid).collect();
        for (set, _) in uid_sets(&new_uids) {
            match self.remove(imap, &set) {
                Ok(()) => {}
                Err(Removal::Unmarked(e)) => {
                    return Err(stop(result, format!("Messages deleted from \"{name}\" are still in \"{trash_name}\": {e}"), &e.into()));
                }
                Err(Removal::Marked(e)) => {
                    let e = NetError::from(e);
                    if e.is_fatal() {
                        return Err(stop(result, format!("Couldn't bring \"{trash_name}\" up to date: {e}"), &e));
                    }
                }
            }
        }
        outcome
    }

    /// Mark the messages of `set` as deleted and, on a server that can remove single messages,
    /// remove them. Never the EXPUNGE that takes every marked message of the mailbox.
    fn remove(&mut self, imap: &mut Imap<Stream<'_>>, set: &str) -> Result<(), Removal> {
        imap.uid_store_set(set, true, &["\\Deleted"]).map_err(Removal::Unmarked)?;
        if self.uidplus {
            imap.uid_expunge(set).map_err(Removal::Marked)?;
        }
        Ok(())
    }

    /// The server mailbox the messages of the move `op` go to; a folder that only exists here
    /// gets its mailbox made first. `Ok(None)`: there is no having it for now, which is reported
    /// once however many messages wait for it. `Err`: the connection is gone.
    fn destination(&mut self, imap: &mut Imap<Stream<'_>>, op: &RemoteOp, result: &mut SyncResult) -> Result<Option<Destination>, bool> {
        let RemoteOp::Move { to, to_folder, create, .. } = op else { return Ok(None) };
        self.destination_of(imap, to, *to_folder, create, result)
    }

    /// The mailbox for local folder `to_folder`, as [`RemoteOp::Move`] names it (`to` and
    /// `create`), worked out once a run: made when it isn't on the server yet.
    fn destination_of(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        to: &str,
        to_folder: Id,
        create: &[String],
        result: &mut SyncResult,
    ) -> Result<Option<Destination>, bool> {
        if let Some(known) = self.destinations.get(&to_folder) {
            return Ok(known.clone());
        }
        let found = match self.find_destination(imap, to, to_folder, create, result) {
            Ok(destination) => Some(destination),
            Err((line, alive)) => {
                result.errors.push(line);
                if !alive {
                    return Err(false);
                }
                None
            }
        };
        self.destinations.insert(to_folder, found.clone());
        Ok(found)
    }

    /// Put the messages moved or copied into the account from elsewhere into their mailboxes,
    /// each whole, with its flags and the time it arrived (#451), and carrying its marker
    /// ([`PLACE_HEADER`]). What the server took is noted (`placed`), with where it went: as the
    /// server says (`APPENDUID`), or else as a look in the mailbox for the copy that carries the
    /// marker finds; what it wouldn't take too (`unplaced`), and said. A copy put there by an
    /// earlier run whose answer was lost, or by one that ended before what it brought back was
    /// kept, is found there the same way and taken for the message's, which is not put there
    /// twice. Only a message that carries the marker is ever taken for the copy: the server's own
    /// message, the same mail come to this account as well, never is. A message whose answer is
    /// lost with the connection is neither placed nor unplaced, and the next run looks for it
    /// there; the ones after it never set out. Returns whether the connection is still good.
    fn places(&mut self, imap: &mut Imap<Stream<'_>>, result: &mut SyncResult) -> bool {
        let request = self.request;
        for place in request.places.iter().take(MAX_PLACES) {
            let ImapPlace { message, to, to_folder, create, marker } = place;
            let destination = match self.destination_of(imap, to, *to_folder, create, result) {
                Ok(Some(destination)) => destination,
                // Said already: it waits.
                Ok(None) => continue,
                Err(alive) => return alive,
            };
            let given = RemoteFlags { draft: false, ..RemoteFlags::of(message, 0) };
            // A copy without a marker that can be written (the engine always gives one) can't be
            // told from other mail there: it is only ever where the server says it put it.
            let marker = Some(marker.as_str()).filter(|m| marked(m));
            // What the copy is looked for by: its `Message-ID`, which servers are asked for alike,
            // or else the marker itself.
            let id = mailcraft_mime::written_message_id(&message.message_id).filter(|id| searchable(id));
            let key = marker.map(|m| id.map_or((PLACE_HEADER, m.to_string()), |id| ("Message-ID", id)));
            // Said, for the run ends there: a server that cuts off the same look every time keeps
            // the account's new mail away until the message is moved out of it.
            let looked =
                |e: &NetError| format!("Couldn't look for \"{}\" in \"{}\" on the server: {e}", subject_of(message), shown(&destination.remote));
            let before = match self.copies(imap, &destination, *to_folder, key.as_ref()) {
                Ok(before) => before,
                Err(e) => return stop(result, looked(&e), &e),
            };
            if let (Some((uid_validity, uids)), Some(marker)) = (&before, marker) {
                match self.carrying(imap, uids, marker) {
                    Ok(Some((uid, flags))) => {
                        // Put there before: it is the message's copy, with the flags it has there
                        // (what changed here since goes up next).
                        self.arrived.entry(destination.remote.clone()).or_default().push((*uid_validity, uid));
                        result.placed.push(Placed { message: message.id, to_folder: *to_folder, uid, uid_validity: *uid_validity, flags });
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => return stop(result, looked(&e), &e),
                }
            }
            let mut flags = Vec::new();
            if given.read {
                flags.push("\\Seen");
            }
            if given.flagged {
                flags.push("\\Flagged");
            }
            if given.answered {
                flags.push("\\Answered");
            }
            let received = if message.received > 0 { message.received } else { message.date };
            let date = crate::sync::imap_date_time(received);
            // Without who got a blind copy: the copy is for anyone else who can open the mailbox
            // to see too.
            let options = WriteOptions { include_bcc: false, offset_minutes: mailcraft_time::local_offset_minutes(message.date) };
            let written = match marker {
                Some(marker) => mailcraft_mime::write_placed(message, &options, marker),
                None => mailcraft_mime::write(message, &options),
            };
            match imap.append_at(&destination.raw, &flags, date.as_deref(), &written) {
                Ok(appended) => {
                    let (mut uid_validity, mut uid) = appended.map_or((0, 0), |a| (a.uid_validity, a.uid));
                    if uid == 0
                        && let Some(marker) = marker
                    {
                        // The server didn't say where it went: to the copy that carries the marker.
                        // It is there, and where is found by the next run when this look fails, for
                        // that run looks for it before it puts it there again.
                        match self.copies(imap, &destination, *to_folder, key.as_ref()) {
                            Ok(Some((validity, now))) => match self.carrying(imap, &now, marker) {
                                Ok(Some((found, _))) => (uid_validity, uid) = (validity, found),
                                Ok(None) => {}
                                Err(e) => return stop(result, looked(&e), &e),
                            },
                            Ok(None) => {}
                            Err(e) => return stop(result, looked(&e), &e),
                        }
                    }
                    if uid != 0 {
                        self.arrived.entry(destination.remote.clone()).or_default().push((uid_validity, uid));
                    }
                    result.placed.push(Placed { message: message.id, to_folder: *to_folder, uid, uid_validity, flags: given });
                }
                Err(e) => {
                    let e = NetError::from(e);
                    result.errors.push(format!("Couldn't put \"{}\" in \"{}\" on the server: {e}", subject_of(message), shown(&destination.remote)));
                    if e.is_fatal() {
                        // The connection went while it was on its way: it may have got there all
                        // the same, and the next run looks for it there.
                        return false;
                    }
                    result.unplaced.push(message.id);
                }
            }
        }
        true
    }

    /// The messages in the destination's mailbox whose header field `key.0` has `key.1` in it (a
    /// message's `Message-ID` as it is written, or a copy's marker) that the mailbox here doesn't
    /// know (no message of the folder has that UID) and this run hasn't put there, with the
    /// mailbox's `UIDVALIDITY`. `Ok(None)`: they can't be looked for (no key, one that can't be
    /// asked for, the server won't say, or the mailbox is numbered anew); `Err`: the connection
    /// went, and why.
    fn copies(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        destination: &Destination,
        to_folder: Id,
        key: Option<&(&str, String)>,
    ) -> Result<Option<(u32, Vec<u32>)>, NetError> {
        // One the server can't be asked for (too long, say) isn't looked for.
        let Some((field, text)) = key.filter(|(_, text)| searchable(text)) else { return Ok(None) };
        let uid_validity = match &self.searching {
            Some((raw, uid_validity)) if *raw == destination.raw => *uid_validity,
            _ => {
                self.searching = None;
                match imap.select(&destination.raw, true) {
                    Ok(selected) => {
                        self.searching = Some((destination.raw.clone(), selected.uid_validity));
                        selected.uid_validity
                    }
                    Err(e) => return fatal(e).map(|()| None),
                }
            }
        };
        let folder = self.request.folders.iter().find(|f| f.id == to_folder);
        if uid_validity == 0 || folder.is_some_and(|f| f.uid_validity != 0 && f.uid_validity != uid_validity) {
            return Ok(None);
        }
        let found = match imap.uid_search_header(field, text) {
            Ok(found) => found,
            Err(e) => return fatal(e).map(|()| None),
        };
        let known = folder.map_or(&[][..], |f| f.known_uids.as_slice());
        let put = self.arrived.get(&destination.remote).map_or(&[][..], Vec::as_slice);
        let unknown = found.into_iter().filter(|uid| !known.contains(uid) && !put.contains(&(uid_validity, *uid))).collect();
        Ok(Some((uid_validity, unknown)))
    }

    /// Which of `uids`, messages of the open destination, is the copy that carries `marker`: the
    /// newest few are looked at. Another message, however like the copy it is, never is it. With
    /// its UID come the flags the server has of it. `Err`: the connection went, and why.
    fn carrying(&mut self, imap: &mut Imap<Stream<'_>>, uids: &[u32], marker: &str) -> Result<Option<(u32, RemoteFlags)>, NetError> {
        for uid in uids.iter().rev().take(MAX_LOOKED_AT) {
            match imap.uid_fetch_header(*uid) {
                Ok(Some(fetched)) if mailcraft_mime::carries_place(fetched.body.as_deref().unwrap_or_default(), marker) => {
                    return Ok(Some((*uid, crate::sync::remote_flags(&fetched).at(*uid))));
                }
                Ok(_) => {}
                Err(e) => fatal(e)?,
            }
        }
        Ok(None)
    }

    /// See [`destination`](Self::destination). `Err` is what to tell the owner, and whether the
    /// connection is still good.
    fn find_destination(
        &self,
        imap: &mut Imap<Stream<'_>>,
        to: &str,
        to_folder: Id,
        create: &[String],
        result: &mut SyncResult,
    ) -> Result<Destination, (String, bool)> {
        if create.is_empty() {
            let mailbox = self.listed(to).filter(|m| kinds::selectable(m)).ok_or_else(|| gone(to))?;
            return Ok(Destination { raw: mailbox.raw.clone(), remote: to.to_string() });
        }
        if let Some(all) = self.gmail_archive(to_folder) {
            // Gmail's archive: the folder follows all mail from now on, and no label is made.
            result.created.push(Created { folder: to_folder, remote: all.remote.clone() });
            return Ok(all);
        }
        let (raw, remote) = self.name_below(to, create)?;
        if let Some(there) = self.already_there(&raw, &remote, to_folder) {
            // Made meanwhile by another program: it is the folder's mailbox.
            let destination = there?;
            result.created.push(Created { folder: to_folder, remote: destination.remote.clone() });
            return Ok(destination);
        }
        if let Err(e) = imap.create(&raw) {
            let e = NetError::from(e);
            return Err((format!("Couldn't make the folder \"{}\" on the server: {e}", shown(&remote)), !e.is_fatal()));
        }
        result.created.push(Created { folder: to_folder, remote: remote.clone() });
        // So that other mail programs list it too. A server that keeps no such list loses nothing.
        if let Err(e) = imap.subscribe(&raw)
            && NetError::from(e.clone()).is_fatal()
        {
            return Err((format!("Couldn't make the folder \"{}\" on the server: {e}", shown(&remote)), false));
        }
        Ok(Destination { raw, remote })
    }

    /// Where the messages archived here go at Gmail when the local folder `to_folder` is the
    /// Archive and follows no mailbox yet, and the server has none for archived mail: its mailbox
    /// of all mail (`\All`), which moving a message to takes the message's label away, as Gmail's
    /// own Archive does. That mailbox is never read as a folder of its own ([`kinds::plan`] leaves
    /// it out, for it has every message of every other one): it is only where archived messages
    /// are put, and where they are moved, flagged and deleted from by their UIDs there.
    fn gmail_archive(&self, to_folder: Id) -> Option<Destination> {
        let archive = self.request.folders.iter().any(|f| f.id == to_folder && f.kind == FolderKind::Archive && f.remote.is_empty());
        if !self.gmail || !archive || kinds::plan(self.mailboxes, &[]).iter().any(|(_, kind)| *kind == FolderKind::Archive) {
            return None;
        }
        let all = self.mailboxes.iter().find(|m| kinds::selectable(m) && m.has_flag("\\All"))?;
        let remote = kinds::remote_name(all);
        // Another folder here follows it already: an archive can't be told from that folder's mail.
        if remote.is_empty() || self.request.folders.iter().any(|f| f.id != to_folder && f.remote == remote) {
            return None;
        }
        Some(Destination { raw: all.raw.clone(), remote })
    }

    /// The server's mailbox known here as `remote`.
    fn listed(&self, remote: &str) -> Option<&Mailbox> {
        self.mailboxes.iter().find(|m| kinds::remote_name(m) == remote)
    }

    /// The mailbox to make for a folder that only exists here, `create` below `to` (see
    /// [`RemoteOp::Move`]): its name as the server wants it, and as it will be known here.
    fn name_below(&self, to: &str, create: &[String]) -> Result<(String, String), (String, bool)> {
        // Below the nearest folder the server has, in the server's spelling of it and with the
        // server's own separator.
        let (mut raw, delimiter) = if to.is_empty() {
            let inbox = self.mailboxes.iter().find(|m| kinds::is_inbox(m)).and_then(|m| m.delimiter);
            (String::new(), inbox.or_else(|| self.mailboxes.iter().find_map(|m| m.delimiter)).unwrap_or('/'))
        } else {
            let parent = self.listed(to).ok_or_else(|| gone(to))?;
            let Some(delimiter) = parent.delimiter else {
                return Err((format!("Couldn't make a folder inside \"{}\": the server keeps no folders inside others", shown(to)), true));
            };
            (parent.raw.clone(), delimiter)
        };
        let unnamed = || ("Couldn't make a folder on the server: it has no usable name".to_string(), true);
        if create.len() > MAX_LEVELS {
            return Err(unnamed());
        }
        for level in create {
            // A name can't hold the character that parts the levels.
            let level: String = level.chars().filter(|c| !c.is_control()).map(|c| if c == delimiter { '_' } else { c }).collect();
            let level = level.trim();
            if level.is_empty() {
                return Err(unnamed());
            }
            if !raw.is_empty() {
                raw.push(delimiter);
            }
            raw.push_str(&encode_utf7(level));
        }
        // The name the mailbox will be listed under from now on.
        let remote = decode_utf7(&raw);
        Ok((raw, remote))
    }

    /// The mailbox the server already has under the name one would be made with for local folder
    /// `to_folder`; `None` when it has none. It is the folder's, unless it can't be opened or a
    /// folder here follows it already.
    fn already_there(&self, raw: &str, remote: &str, to_folder: Id) -> Option<Result<Destination, (String, bool)>> {
        let existing = self.mailboxes.iter().find(|m| m.raw == raw || kinds::remote_name(m) == remote)?;
        let remote = kinds::remote_name(existing);
        if !kinds::selectable(existing) || self.request.folders.iter().any(|f| f.id != to_folder && f.remote == remote) {
            return Some(Err((format!("Couldn't make the folder \"{}\" on the server: it has another folder of that name", shown(&remote)), true)));
        }
        Some(Ok(Destination { raw: existing.raw.clone(), remote }))
    }

    /// For a move whose message is no longer where it was to be moved from. When the folder it
    /// was moved to only exists here, and the server has the mailbox that would be made for it,
    /// the folder is told so. A run that was cut off after it had made the mailbox and moved
    /// the message leaves just that behind; without the tie, what comes down from the mailbox
    /// would be taken for another folder's, and the moved message would show twice.
    fn adopt(&mut self, op: &RemoteOp, result: &mut SyncResult) {
        let RemoteOp::Move { to, to_folder, create, .. } = op else { return };
        if create.is_empty() || self.destinations.contains_key(to_folder) {
            return;
        }
        if let Some(all) = self.gmail_archive(*to_folder) {
            result.created.push(Created { folder: *to_folder, remote: all.remote.clone() });
            self.destinations.insert(*to_folder, Some(all));
            return;
        }
        let Ok((raw, remote)) = self.name_below(to, create) else { return };
        if let Some(Ok(destination)) = self.already_there(&raw, &remote, *to_folder) {
            result.created.push(Created { folder: *to_folder, remote: destination.remote.clone() });
            self.destinations.insert(*to_folder, Some(destination));
        }
    }
}

/// What the owner is told when the mailbox that messages were to go to isn't there.
fn gone(remote: &str) -> (String, bool) {
    (format!("Couldn't move messages to \"{}\": the folder is no longer on the server", shown(remote)), true)
}

/// Why messages weren't removed.
enum Removal {
    /// They couldn't be marked as deleted: they are as they were.
    Unmarked(ProtoError),
    /// They are marked as deleted, and still there.
    Marked(ProtoError),
}
