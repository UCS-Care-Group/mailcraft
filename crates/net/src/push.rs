//! Pushing the changes made here to the IMAP server: flags, categories (as keywords), moves and
//! deletions ([`SyncRequest::ops`]), one source mailbox at a time, before anything is downloaded;
//! and before them, the messages here that the server doesn't have yet ([`uploads`]).
//!
//! Mail is never lost to a push. A message leaves a mailbox only by UID MOVE, or, on a server
//! without it, once its copy is in the other mailbox. A message is removed for good only when
//! the owner deleted it, and then by its UID (UID EXPUNGE, RFC 4315); a server that can't do
//! that keeps it marked `\Deleted`, for the mailbox-wide EXPUNGE would take what other programs
//! marked too. A change is only made in a mailbox whose `UIDVALIDITY` is the one the UIDs were
//! learned under, and when a step fails the rest of that mailbox's changes wait for the next run.

use std::collections::{BTreeMap, HashMap, HashSet};

use mailcraft_model::sync::{
    CategoriesPushed, Created, Deleted, FlagsPushed, FolderOp, FolderPushed, Moved, RemoteFlags, RemoteOp, SyncRequest, SyncResult, Upload, Uploaded,
    category_keyword, renamed_path,
};
use mailcraft_model::{FlagStatus, FolderKind, Id};
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::{Copied, Imap, Mailbox, Selected, decode_utf7, encode_utf7, uid_sets};

use mailcraft_mime::WriteOptions;

use crate::sync::{Run, label, open_imap_with, shown};
use crate::transport::Stream;
use crate::{NetError, kinds};

/// Most changes pushed in one run; the rest waits for the next one.
const MAX_OPS: usize = 10_000;
/// Deepest a mailbox is made below one the server has.
const MAX_LEVELS: usize = 32;

/// Messages that moves of this run put into mailboxes, by the mailbox's name here: the
/// `UIDVALIDITY` the server gave with their new UIDs, and those UIDs. The download takes them
/// for known instead of fetching them again.
pub(crate) type Arrived = HashMap<String, Vec<(u32, u32)>>;

/// Push `request`'s changes (the run's own, or those made here while it runs). What went through is noted in the run's answer (`flagged`,
/// `moved`, `deleted`, `created`), what didn't in its `errors`, and handed over after each
/// mailbox: the sooner the mailbox here knows a change was made, the less there is for a run
/// that ends early to find done. `None` when the connection was lost.
///
/// A mailbox's changes are made together: a run that is called off stops between two mailboxes,
/// never between a copy and the removal of its original.
pub(crate) fn push(run: &mut Run, request: &SyncRequest, imap: &mut Imap<Stream<'_>>, mailboxes: &[Mailbox]) -> Option<Arrived> {
    let ops: Vec<&RemoteOp> = request.ops.iter().take(MAX_OPS).collect();
    let mut push = Push { request, mailboxes, can_move: false, uidplus: false, destinations: HashMap::new(), arrived: Arrived::new() };
    if ops.iter().any(|op| !matches!(op, RemoteOp::Flags { .. } | RemoteOp::Categories { .. })) {
        // Asked now: what a server can do is often only told once it knows who is asking.
        match imap.capabilities() {
            Ok(capabilities) => {
                push.can_move = capabilities.iter().any(|c| c == "MOVE");
                push.uidplus = capabilities.iter().any(|c| c == "UIDPLUS");
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
            run.hand_over(format!("Updated {} on the server", label(remote)));
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
    /// The destination worked out for each local folder; `None` for one that can't be had (the
    /// reason was reported once).
    destinations: HashMap<Id, Option<Destination>>,
    arrived: Arrived,
}

/// Note a problem that ends the work on a mailbox; answers whether the connection is still good.
fn stop(result: &mut SyncResult, line: String, error: &NetError) -> bool {
    result.errors.push(line);
    !error.is_fatal()
}

fn has(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
}

impl Push<'_> {
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
        match self.changes(imap, remote, &name, &selected, ops, result) {
            Ok(()) => true,
            Err(alive) => alive,
        }
    }

    /// Flags and categories first, then moves, then deletions. `Err` ends the work on this
    /// mailbox, with whether the connection is still good.
    fn changes(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        remote: &str,
        name: &str,
        selected: &Selected,
        ops: &[&RemoteOp],
        result: &mut SyncResult,
    ) -> Result<(), bool> {
        let exists = selected.exists;
        self.flags(imap, remote, name, ops, result)?;
        self.categories(imap, remote, name, selected, ops, result)?;
        let leaving: Vec<u32> = ops
            .iter()
            .filter_map(|op| match op {
                RemoteOp::Move { uid, .. } | RemoteOp::Delete { uid, .. } => Some(*uid),
                RemoteOp::Flags { .. } | RemoteOp::Categories { .. } => None,
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

    /// Categories given and taken here, as keywords: one UID STORE per keyword and direction.
    /// A mailbox that keeps no keywords of the owner's choosing (Exchange Online says so in its
    /// `PERMANENTFLAGS`) is told nothing, and the answer says so, once, without a word of
    /// complaint: the categories stay here.
    fn categories(
        &mut self,
        imap: &mut Imap<Stream<'_>>,
        remote: &str,
        name: &str,
        selected: &Selected,
        ops: &[&RemoteOp],
        result: &mut SyncResult,
    ) -> Result<(), bool> {
        let wanted: Vec<(u32, &Vec<String>, &Vec<String>)> = ops
            .iter()
            .filter_map(|op| match op {
                RemoteOp::Categories { uid, add, remove, .. } => Some((*uid, add, remove)),
                _ => None,
            })
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        if !selected.keeps_keywords() {
            if !result.keywords_refused.iter().any(|r| r == remote) {
                result.keywords_refused.push(remote.to_string());
            }
            return Ok(());
        }
        // Keyword, on or off, and the messages; the names as they are here go with each.
        let mut changes: Vec<(String, bool, Vec<u32>)> = Vec::new();
        let mut asked: BTreeMap<u32, (Vec<String>, Vec<String>)> = BTreeMap::new();
        for (uid, add, remove) in wanted {
            for (names, on) in [(add, true), (remove, false)] {
                for category in names {
                    // A name that can't be a keyword stays here (the mailbox only asks for those that can).
                    let Some(keyword) = category_keyword(category) else { continue };
                    let entry = asked.entry(uid).or_default();
                    if on {
                        entry.0.push(category.clone())
                    } else {
                        entry.1.push(category.clone())
                    }
                    match changes.iter_mut().find(|(k, o, _)| *k == keyword && *o == on) {
                        Some((_, _, uids)) => uids.push(uid),
                        None => changes.push((keyword, on, vec![uid])),
                    }
                }
            }
        }
        let mut done: HashSet<(u32, String, bool)> = HashSet::new();
        let mut outcome = Ok(());
        'changes: for (keyword, on, uids) in &changes {
            for (set, uids) in uid_sets(uids) {
                if let Err(e) = imap.uid_store_set(&set, *on, &[keyword.as_str()]) {
                    outcome = Err(stop(result, format!("Couldn't change categories in \"{name}\": {e}"), &e.into()));
                    break 'changes;
                }
                done.extend(uids.iter().map(|uid| (*uid, keyword.clone(), *on)));
            }
        }
        // What went through, message by message.
        for (uid, (add, remove)) in asked {
            let went = |names: Vec<String>, on: bool| -> Vec<String> {
                names.into_iter().filter(|n| category_keyword(n).is_some_and(|k| done.contains(&(uid, k, on)))).collect()
            };
            let (add, remove) = (went(add, true), went(remove, false));
            if !add.is_empty() || !remove.is_empty() {
                result.categorized.push(CategoriesPushed { folder: remote.to_string(), uid, add, remove });
            }
        }
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
        if let Some(known) = self.destinations.get(to_folder) {
            return Ok(known.clone());
        }
        let found = match self.find_destination(imap, to, *to_folder, create, result) {
            Ok(destination) => Some(destination),
            Err((line, alive)) => {
                result.errors.push(line);
                if !alive {
                    return Err(false);
                }
                None
            }
        };
        self.destinations.insert(*to_folder, found.clone());
        Ok(found)
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

/// Most messages put on the server in one run, and most bytes of them; the rest waits for the
/// next one.
const MAX_UPLOADS: usize = 200;
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
/// The largest message put on the server from here.
const MAX_UPLOAD: usize = 25 * 1024 * 1024;

/// Put on the server the messages here that their folders' mailboxes don't have yet
/// ([`SyncRequest::uploads`]): a copy of a message the server has is made there (UID COPY), any
/// other is put there (APPEND), drafts marked as drafts. What went is noted in the run's answer
/// (`uploaded`), with the message's UID when the server said; what didn't, in its `errors`.
/// `None` when the connection was lost.
///
/// Nothing goes up twice: before a message is put there, the mailbox is asked for one with its
/// Message-ID that no message here has (a run that was cut off after it put the message there,
/// mail that was there already), and that one is taken for it.
pub(crate) fn uploads(run: &mut Run, request: &SyncRequest, imap: &mut Imap<Stream<'_>>, mailboxes: &[Mailbox]) -> Option<Arrived> {
    let mut arrived = Arrived::new();
    let mut bytes = 0usize;
    let mut todo: Vec<&Upload> = Vec::new();
    for upload in request.uploads.iter().take(MAX_UPLOADS) {
        bytes = bytes.saturating_add(upload.message.size() as usize);
        if bytes > MAX_UPLOAD_BYTES && !todo.is_empty() {
            break;
        }
        todo.push(upload);
    }
    if todo.is_empty() || run.cancelled() {
        return Some(arrived);
    }
    let mut work = Uploads { request, mailboxes, arrived: &mut arrived };
    // Copies first, by the mailbox they are copied from; then the rest, by where they go.
    let (copies, puts): (Vec<&Upload>, Vec<&Upload>) = todo.into_iter().partition(|u| !u.copy_from.is_empty() && u.copy_uid != 0);
    let mut put_after: Vec<&Upload> = Vec::new();
    for from in in_order(copies.iter().map(|u| u.copy_from.as_str())) {
        if run.cancelled() {
            return Some(arrived_of(work));
        }
        let here: Vec<&Upload> = copies.iter().copied().filter(|u| u.copy_from == from).collect();
        let (alive, rest) = work.copies(imap, from, &here, &mut run.out);
        put_after.extend(rest);
        if run.out.has_news() || !run.out.errors.is_empty() {
            run.hand_over(format!("Updated {} on the server", label(from)));
        }
        if !alive {
            return None;
        }
    }
    let puts: Vec<&Upload> = puts.into_iter().chain(put_after).collect();
    for to in in_order(puts.iter().map(|u| u.to.as_str())) {
        if run.cancelled() {
            break;
        }
        let here: Vec<&Upload> = puts.iter().copied().filter(|u| u.to == to).collect();
        let alive = work.puts(imap, to, &here, &mut run.out);
        if run.out.has_news() || !run.out.errors.is_empty() {
            run.hand_over(format!("Updated {} on the server", label(to)));
        }
        if !alive {
            return None;
        }
    }
    Some(arrived_of(work))
}

/// The names, each once, in the order they first come.
fn in_order<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen: HashSet<&str> = HashSet::new();
    names.filter(|name| seen.insert(name)).collect()
}

fn arrived_of(work: Uploads<'_, '_>) -> Arrived {
    std::mem::take(work.arrived)
}

/// The messages of a run being put on the server.
struct Uploads<'a, 'b> {
    request: &'a SyncRequest,
    mailboxes: &'a [Mailbox],
    /// Where they went, for the download to take them for known.
    arrived: &'b mut Arrived,
}

/// The flags a message goes to the server with.
fn upload_flags(upload: &Upload) -> (Vec<&'static str>, RemoteFlags) {
    let m = &upload.message;
    let flags = RemoteFlags { uid: 0, read: m.read || m.draft, flagged: m.flag == FlagStatus::Flagged, answered: m.answered, draft: m.draft };
    let mut words = Vec::new();
    for (on, word) in [(flags.read, "\\Seen"), (flags.flagged, "\\Flagged"), (flags.answered, "\\Answered"), (flags.draft, "\\Draft")] {
        if on {
            words.push(word);
        }
    }
    (words, flags)
}

/// A subject for a line to the owner.
fn subject(upload: &Upload) -> String {
    let words: Vec<&str> = upload.message.subject.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let subject: String = words.join(" ").chars().take(60).collect();
    if subject.is_empty() { "(no subject)".into() } else { subject }
}

impl Uploads<'_, '_> {
    /// The server's mailbox known here as `remote`, when it has it and it can be opened.
    fn listed(&self, remote: &str) -> Option<&Mailbox> {
        self.mailboxes.iter().find(|m| kinds::selectable(m) && kinds::remote_name(m) == remote)
    }

    /// Note an upload as done.
    fn done(&mut self, upload: &Upload, uid: u32, uid_validity: u32, flags: RemoteFlags, copied: bool, message_id: String, result: &mut SyncResult) {
        let uid_validity = if uid == 0 { 0 } else { uid_validity };
        if uid != 0 && uid_validity != 0 {
            self.arrived.entry(upload.to.clone()).or_default().push((uid_validity, uid));
        }
        result.uploaded.push(Uploaded {
            message: upload.message.id,
            folder: upload.message.folder,
            to: upload.to.clone(),
            uid,
            uid_validity,
            message_id,
            flags: flags.at(uid),
            copied,
        });
    }

    /// Copy the messages `uploads` were copied from, out of the mailbox `from`. Answers whether
    /// the connection is still good, and the ones that have to be put there from here after all
    /// (what they were copied from isn't there any more).
    fn copies<'u>(&mut self, imap: &mut Imap<Stream<'_>>, from: &str, uploads: &[&'u Upload], result: &mut SyncResult) -> (bool, Vec<&'u Upload>) {
        let name = shown(from);
        let learned_under = self.request.folders.iter().find(|f| f.remote == from).map_or(0, |f| f.uid_validity);
        let Some(source) = self.listed(from) else {
            return (true, uploads.iter().copied().filter(|u| !u.copy_only).collect());
        };
        let selected = match imap.select(&source.raw, true) {
            Ok(selected) => selected,
            Err(e) => return (stop(result, format!("Couldn't open \"{name}\" to copy from it: {e}"), &e.into()), Vec::new()),
        };
        if learned_under == 0 || selected.uid_validity != learned_under {
            // The UIDs are of another life of the mailbox: nothing is copied by them.
            return (true, uploads.iter().copied().filter(|u| !u.copy_only).collect());
        }
        let uids: Vec<u32> = uploads.iter().map(|u| u.copy_uid).collect();
        let mut present: HashSet<u32> = HashSet::new();
        for (set, _) in uid_sets(&uids) {
            match imap.uid_fetch_flags(&set) {
                Ok(listed) => present.extend(listed.iter().filter(|m| !has(&m.flags, "\\Deleted")).map(|m| m.uid)),
                Err(ProtoError::Rejected(_)) if selected.exists == 0 => {}
                Err(e) => return (stop(result, format!("Couldn't read \"{name}\": {e}"), &e.into()), Vec::new()),
            }
        }
        let mut rest = Vec::new();
        for upload in uploads {
            if !present.contains(&upload.copy_uid) {
                if upload.copy_only {
                    result
                        .errors
                        .push(format!("Couldn't copy \"{}\" on the server: the message it was copied from is no longer there", subject(upload)));
                } else {
                    rest.push(*upload);
                }
                continue;
            }
            // A folder the server doesn't list (any more): the message waits here, and the
            // download of this run tells the mailbox when the folder is gone from the server.
            let Some(to) = self.listed(&upload.to).map(|m| m.raw.clone()) else { continue };
            match imap.uid_copy(&upload.copy_uid.to_string(), &to) {
                Ok(copied) => {
                    let (validity, uid) = copied
                        .and_then(|c| c.uids.iter().find(|(old, _)| *old == upload.copy_uid).map(|(_, new)| (c.uid_validity, *new)))
                        .unwrap_or((0, 0));
                    let (_, flags) = upload_flags(upload);
                    self.done(upload, uid, validity, flags, true, upload.message.message_id.clone(), result);
                }
                Err(e) => {
                    let line = format!("Couldn't copy \"{}\" on the server: {e}", subject(upload));
                    if !stop(result, line, &e.into()) {
                        return (false, Vec::new());
                    }
                }
            }
        }
        (true, rest)
    }

    /// Put the messages `uploads` into the mailbox `to`. Answers whether the connection is still good.
    fn puts(&mut self, imap: &mut Imap<Stream<'_>>, to: &str, uploads: &[&Upload], result: &mut SyncResult) -> bool {
        let name = shown(to);
        // A folder the server doesn't list (any more): its messages wait here, and the download
        // of this run tells the mailbox when the folder is gone from the server.
        let Some(mailbox) = self.listed(to) else {
            log::debug!("{name}: not on the server, nothing put there");
            return true;
        };
        let raw = mailbox.raw.clone();
        // Opened to look for what is there already; nothing in it is changed.
        let selected = match imap.select(&raw, true) {
            Ok(selected) => selected,
            Err(e) => return stop(result, format!("Couldn't open \"{name}\": {e}"), &e.into()),
        };
        let state = self.request.folders.iter().find(|f| f.remote == to);
        // UIDs that messages here have, or that are on their way out: never another message's copy.
        let mut taken: HashSet<u32> =
            state.filter(|f| f.uid_validity == selected.uid_validity).map(|f| f.known_uids.iter().copied().collect()).unwrap_or_default();
        taken.extend(self.request.ops.iter().filter_map(|op| match op {
            RemoteOp::Delete { from, uid } if from == to => Some(*uid),
            _ => None,
        }));
        for upload in uploads {
            let mut message = upload.message.clone();
            if message.message_id.trim().is_empty() {
                // Known by it when it comes back down.
                let domain = self.request.account.email.rsplit_once('@').map_or("", |(_, d)| d);
                message.message_id = mailcraft_mime::new_message_id(domain, message.date.max(message.received), message.id);
            }
            let (words, flags) = upload_flags(upload);
            // There already (a run cut off after it put it there, mail that was there before).
            match imap.uid_search_message_id(&message.message_id) {
                Ok(found) => {
                    if let Some(uid) = found.into_iter().rev().find(|uid| !taken.contains(uid)) {
                        taken.insert(uid);
                        self.done(upload, uid, selected.uid_validity, flags, false, message.message_id.clone(), result);
                        continue;
                    }
                }
                Err(e) => {
                    let e = NetError::from(e);
                    if e.is_fatal() {
                        return stop(result, format!("Couldn't read \"{name}\": {e}"), &e);
                    }
                }
            }
            if upload.copy_only {
                continue;
            }
            let options = WriteOptions { include_bcc: true, offset_minutes: mailcraft_time::local_offset_minutes(message.date) };
            let bytes = mailcraft_mime::write(&message, &options);
            if bytes.len() > MAX_UPLOAD {
                result.errors.push(format!("Couldn't put \"{}\" into \"{name}\" on the server: it is too large", subject(upload)));
                continue;
            }
            match imap.append_uid(&raw, &words, &bytes) {
                Ok(at) => {
                    let (validity, uid) = at.unwrap_or((0, 0));
                    if uid != 0 {
                        taken.insert(uid);
                    }
                    self.done(upload, uid, validity, flags, false, message.message_id.clone(), result);
                }
                Err(e) => {
                    let line = format!("Couldn't put \"{}\" into \"{name}\" on the server: {e}", subject(upload));
                    if !stop(result, line, &e.into()) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// Most folder changes made in one run; the rest waits for the next one.
const MAX_FOLDER_OPS: usize = 1_000;
/// Longest name a folder made or renamed on the server gets, in characters.
const MAX_FOLDER_NAME: usize = 255;

/// Make, rename, move and delete on the server the folders the owner made, renamed, moved and
/// deleted here ([`SyncRequest::folder_ops`]), over a connection of its own, before the rest of
/// the run. What went through, and what the server refused, is noted in the run's answer
/// (`folders_pushed`); a change that wasn't tried (the connection was lost) is asked for again.
///
/// Returns the request as the rest of the run is to see it: the mailboxes known by the names
/// they have now, and the folders that have a mailbox now tied to it. `None` when there was
/// nothing to do or no server to do it on.
///
/// Nothing is deleted on the server but a mailbox inside its Deleted Items, as Outlook has it:
/// deleting a folder here moves its mailbox there (RENAME), and only deleting it from there
/// deletes it (DELETE).
pub(crate) fn folders(run: &mut Run) -> Option<SyncRequest> {
    let request = run.request;
    if request.folder_ops.is_empty() || request.account.incoming.is_none() || run.cancelled() {
        return None;
    }
    run.hand_over("Changing folders on the server".into());
    if run.cancelled() {
        return None;
    }
    // Not called off half way: a change is made whole or not at all.
    let mut imap = match open_imap_with(&request.account, request.secret(), &run.limits.patience, None) {
        Ok(imap) => imap,
        Err(e) => {
            run.out.errors.push(format!("Couldn't change folders on the server: {e}"));
            return None;
        }
    };
    let mailboxes = match imap.list() {
        Ok(list) => list,
        Err(e) => {
            run.out.errors.push(format!("Couldn't change folders on the server: {e}"));
            imap.get_mut().leave();
            let _ = imap.logout();
            return None;
        }
    };
    let delimiter = mailboxes.iter().find(|m| kinds::is_inbox(m)).and_then(|m| m.delimiter).or_else(|| mailboxes.iter().find_map(|m| m.delimiter));
    let mut work = FolderWork { request, mailboxes, delimiter, renames: Vec::new(), created: Vec::new() };
    for op in request.folder_ops.iter().take(MAX_FOLDER_OPS) {
        if !work.apply(&mut imap, op, &mut run.out) {
            // The connection is gone: the rest waits for the next run.
            break;
        }
    }
    imap.get_mut().leave();
    let _ = imap.logout();
    run.hand_over("Changed folders on the server".into());
    Some(work.reshaped())
}

/// The folder changes of a run under way.
struct FolderWork<'a> {
    request: &'a SyncRequest,
    /// The server's mailboxes, as they are after the changes made so far.
    mailboxes: Vec<Mailbox>,
    /// What parts the levels of the server's mailbox names; `None` for a server without levels.
    delimiter: Option<char>,
    /// The renames made, in order: from, to (names here).
    renames: Vec<(String, String)>,
    /// Local folders that have a mailbox now, and its name here.
    created: Vec<(Id, String)>,
}

/// What a folder change came to.
enum Outcome {
    /// Done: the mailbox's name here now (empty for one deleted).
    Done(String),
    /// Not done, and not to be asked for again: what to tell the owner.
    Refused(String),
}

impl FolderWork<'_> {
    fn delimiter_text(&self) -> String {
        self.delimiter.map(String::from).unwrap_or_default()
    }

    /// `path` as it is called after the renames made so far.
    fn current(&self, path: &str) -> String {
        let delimiter = self.delimiter_text();
        let mut path = path.to_string();
        for (from, to) in &self.renames {
            if let Some(moved) = renamed_path(&path, from, to, &delimiter) {
                path = moved;
            }
        }
        path
    }

    /// The mailbox known here as `remote`.
    fn find(&self, remote: &str) -> Option<&Mailbox> {
        self.mailboxes.iter().find(|m| kinds::remote_name(m) == remote)
    }

    /// One level of a name as the server wants it: one line, without the character that parts
    /// the levels, in modified UTF-7. `None` when nothing usable is left.
    fn level(&self, name: &str) -> Option<String> {
        let level: String =
            name.chars().filter(|c| !c.is_control()).map(|c| if Some(c) == self.delimiter { '_' } else { c }).take(MAX_FOLDER_NAME).collect();
        let level = level.trim();
        (!level.is_empty()).then(|| encode_utf7(level))
    }

    /// The raw name of `level` inside the mailbox `parent_raw` (empty: the top level).
    fn join(&self, parent_raw: &str, level: &str) -> Result<String, String> {
        if parent_raw.is_empty() {
            return Ok(level.to_string());
        }
        let delimiter = self.delimiter.ok_or_else(|| "the server keeps no folders inside others".to_string())?;
        Ok(format!("{parent_raw}{delimiter}{level}"))
    }

    /// The raw name of the mailbox known here as `parent` (empty: the top level).
    fn parent_raw(&self, parent: &str) -> Result<String, String> {
        if parent.is_empty() {
            return Ok(String::new());
        }
        let parent = self.current(parent);
        self.find(&parent).map(|m| m.raw.clone()).ok_or_else(|| format!("the folder \"{}\" it goes into is not on the server", shown(&parent)))
    }

    /// The server's mailbox by the raw name `raw`, or by the name it would have here.
    fn taken(&self, raw: &str) -> Option<&Mailbox> {
        let remote = decode_utf7(raw);
        self.mailboxes.iter().find(|m| m.raw.eq_ignore_ascii_case(raw) || kinds::remote_name(m) == remote)
    }

    /// Make the change `op` asks for. Returns whether the connection is still good.
    fn apply(&mut self, imap: &mut Imap<Stream<'_>>, op: &FolderOp, result: &mut SyncResult) -> bool {
        let (folder, from, made) = match op {
            FolderOp::Create { folder, parent, name } => (*folder, String::new(), self.create(imap, *folder, parent, name)),
            FolderOp::Rename { folder, from, parent, name } => {
                let from = self.current(from);
                let made = self.rename(imap, &from, parent.as_deref(), name);
                (*folder, from, made)
            }
            FolderOp::Delete { remote } => {
                let remote = self.current(remote);
                let made = self.delete(imap, &remote);
                (0, remote, made)
            }
        };
        let delimiter = self.delimiter_text();
        match made {
            Ok(Outcome::Done(to)) => {
                if matches!(op, FolderOp::Create { .. }) {
                    self.created.push((folder, to.clone()));
                }
                result.folders_pushed.push(FolderPushed { folder, from, to, delimiter, refused: false });
                true
            }
            Ok(Outcome::Refused(line)) => {
                result.errors.push(line);
                result.folders_pushed.push(FolderPushed { folder, from, to: String::new(), delimiter, refused: true });
                true
            }
            Err(line) => {
                result.errors.push(line);
                false
            }
        }
    }

    /// See [`FolderOp::Create`]. `Err`: the connection is gone, and what to tell the owner.
    fn create(&mut self, imap: &mut Imap<Stream<'_>>, folder: Id, parent: &str, name: &str) -> Result<Outcome, String> {
        let what = format!("Couldn't make the folder \"{}\" on the server", shown(name));
        let Some(level) = self.level(name) else { return Ok(Outcome::Refused(format!("{what}: it has no usable name"))) };
        let raw = match self.parent_raw(parent).and_then(|parent| self.join(&parent, &level)) {
            Ok(raw) => raw,
            Err(why) => return Ok(Outcome::Refused(format!("{what}: {why}"))),
        };
        if let Some(there) = self.taken(&raw) {
            // Made meanwhile by another program: it is the folder's mailbox, unless a folder here
            // follows it already.
            let remote = kinds::remote_name(there);
            if kinds::selectable(there) && !self.request.folders.iter().any(|f| f.id != folder && f.remote == remote) {
                return Ok(Outcome::Done(remote));
            }
            return Ok(Outcome::Refused(format!("{what}: it has another folder of that name")));
        }
        if let Err(e) = imap.create(&raw) {
            return failed(e, what);
        }
        // So that other mail programs list it too. A server that keeps no such list loses nothing.
        if let Err(e) = imap.subscribe(&raw)
            && NetError::from(e.clone()).is_fatal()
        {
            return Err(format!("{what}: {e}"));
        }
        let remote = decode_utf7(&raw);
        self.mailboxes.push(Mailbox { name: remote.clone(), raw, delimiter: self.delimiter, flags: Vec::new() });
        Ok(Outcome::Done(remote))
    }

    /// See [`FolderOp::Rename`].
    fn rename(&mut self, imap: &mut Imap<Stream<'_>>, from: &str, parent: Option<&str>, name: &str) -> Result<Outcome, String> {
        let what = format!("Couldn't move or rename the folder \"{}\" on the server", shown(from));
        let Some(source) = self.find(from).filter(|m| !kinds::is_inbox(m)) else {
            return Ok(Outcome::Refused(format!("{what}: it is no longer there")));
        };
        let source_raw = source.raw.clone();
        let parent_raw = match parent {
            // Where it lies now.
            None => self.delimiter.and_then(|d| source_raw.rsplit_once(d)).map(|(above, _)| above.to_string()).unwrap_or_default(),
            Some(parent) => match self.parent_raw(parent) {
                Ok(raw) => raw,
                Err(why) => return Ok(Outcome::Refused(format!("{what}: {why}"))),
            },
        };
        let Some(level) = self.level(name) else { return Ok(Outcome::Refused(format!("{what}: the new name can't be used"))) };
        let raw = match self.join(&parent_raw, &level) {
            Ok(raw) => raw,
            Err(why) => return Ok(Outcome::Refused(format!("{what}: {why}"))),
        };
        if raw == source_raw {
            return Ok(Outcome::Done(from.to_string()));
        }
        let inside = self.delimiter.map(|d| format!("{source_raw}{d}"));
        if parent_raw == source_raw || inside.as_ref().is_some_and(|inside| parent_raw.starts_with(inside.as_str())) {
            return Ok(Outcome::Refused(format!("{what}: a folder can't go into itself")));
        }
        if self.taken(&raw).is_some() {
            return Ok(Outcome::Refused(format!("{what}: there is a folder of that name there already")));
        }
        if let Err(e) = imap.rename(&source_raw, &raw) {
            return failed(e, what);
        }
        // Other mail programs list it under its new name. A server that keeps no such list, or
        // moved the subscription itself, loses nothing.
        for (name, on) in [(&source_raw, false), (&raw, true)] {
            let said = if on { imap.subscribe(name) } else { imap.unsubscribe(name) };
            if let Err(e) = said
                && NetError::from(e.clone()).is_fatal()
            {
                return Err(format!("{what}: {e}"));
            }
        }
        // It and every mailbox inside it have their new names from here on.
        let delimiter = self.delimiter;
        for mailbox in &mut self.mailboxes {
            let renamed = if mailbox.raw == source_raw {
                Some(raw.clone())
            } else {
                inside.as_ref().and_then(|inside| mailbox.raw.strip_prefix(inside.as_str())).zip(delimiter).map(|(rest, d)| format!("{raw}{d}{rest}"))
            };
            if let Some(renamed) = renamed {
                mailbox.name = decode_utf7(&renamed);
                mailbox.raw = renamed;
            }
        }
        let to = decode_utf7(&raw);
        self.renames.push((from.to_string(), to.clone()));
        Ok(Outcome::Done(to))
    }

    /// See [`FolderOp::Delete`].
    fn delete(&mut self, imap: &mut Imap<Stream<'_>>, remote: &str) -> Result<Outcome, String> {
        let what = format!("Couldn't delete the folder \"{}\" on the server", shown(remote));
        // Gone already: nothing to do.
        let Some(mailbox) = self.find(remote) else { return Ok(Outcome::Done(String::new())) };
        let raw = mailbox.raw.clone();
        // Only ever one inside Deleted Items, and never one a folder here follows.
        let trash = self.request.folders.iter().find(|f| f.kind == FolderKind::Deleted && !f.remote.is_empty()).and_then(|f| self.find(&f.remote));
        let inside_trash =
            trash.zip(self.delimiter).is_some_and(|(trash, d)| raw.strip_prefix(trash.raw.as_str()).is_some_and(|rest| rest.starts_with(d)));
        let followed = self.request.folders.iter().any(|f| f.remote == remote);
        if !inside_trash || followed || kinds::is_inbox(mailbox) {
            return Ok(Outcome::Refused(format!("{what}: it is not in Deleted Items there, so it was deleted here only")));
        }
        if let Err(e) = imap.delete(&raw) {
            return failed(e, what);
        }
        if let Err(e) = imap.unsubscribe(&raw)
            && NetError::from(e.clone()).is_fatal()
        {
            return Err(format!("{what}: {e}"));
        }
        self.mailboxes.retain(|m| m.raw != raw);
        Ok(Outcome::Done(String::new()))
    }

    /// The request as the rest of the run sees it.
    fn reshaped(&self) -> SyncRequest {
        reshape(self.request, &self.renames, &self.created, &self.delimiter_text())
    }
}

/// `request` as the rest of a run is to see it once folders were renamed (`renames`, from and
/// to, names here, in order; `delimiter` parts the levels) and made (`created`: local folders
/// that have a mailbox now, and its name here).
pub(crate) fn reshape(request: &SyncRequest, renames: &[(String, String)], created: &[(Id, String)], delimiter: &str) -> SyncRequest {
    let mut request = request.clone();
    request.folder_ops.clear();
    let rename = |path: &mut String| {
        for (from, to) in renames {
            if let Some(moved) = renamed_path(path, from, to, delimiter) {
                *path = moved;
            }
        }
    };
    for folder in &mut request.folders {
        rename(&mut folder.remote);
        if folder.remote.is_empty()
            && let Some((_, remote)) = created.iter().find(|(id, _)| *id == folder.id)
        {
            folder.remote = remote.clone();
        }
    }
    for upload in &mut request.uploads {
        rename(&mut upload.to);
        rename(&mut upload.copy_from);
    }
    for op in &mut request.ops {
        match op {
            RemoteOp::Flags { folder, .. } => rename(folder),
            RemoteOp::Move { from, to, .. } => {
                rename(from);
                rename(to);
            }
            RemoteOp::Delete { from, .. } => rename(from),
            RemoteOp::Categories { folder, .. } => rename(folder),
        }
    }
    request
}

/// A command the server didn't carry out: refused, or `Err` when the connection is gone.
fn failed(e: ProtoError, what: String) -> Result<Outcome, String> {
    let e = NetError::from(e);
    let line = format!("{what}: {e}");
    if e.is_fatal() { Err(line) } else { Ok(Outcome::Refused(line)) }
}
