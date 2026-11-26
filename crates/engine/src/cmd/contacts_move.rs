// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Move to contacts folder (#1144): one contact or several into another contacts folder, of the
//! same account, of another account, or On this computer.
//!
//! **Never in neither.** Here the move is at once: the contact is in its new folder. Into an
//! account's folder it is made on that server first; the copy a server's folder had is deleted
//! there only once the new one is made (the server answered with its id, or a download showed
//! it). Until then that deletion waits in the queue, held ([`held_deletions`]); when the server
//! won't make the new one, the contact goes back where it was ([`moved_back`]). Onto this computer
//! there is nothing to wait for: the server is told to delete its copy.
//!
//! **One contact or two.** The queue has one note a contact. A contact a server has leaves a new
//! contact in the target, linked to it by [`Contact::moved_from`], and itself goes, as a deletion.
//! One no server has heard of just changes folder. One still to be made on a server (or on its way
//! there, unanswered) isn't moved into another server's folder until the server answers ([`waits`]);
//! one of a selection that waits so is left out and named, and the others move.
//!
//! A move is one command, so one undo step, worked out as any other step's
//! ([`super::contacts_remote::after_undo`]): undone after it went up, each server is told the
//! opposite. A programmatic call never asks; the window's dialog asks about a contact already in
//! the target first ([`already_in`]).

use std::collections::{HashMap, HashSet};

use mailcraft_model::contact_sync::ContactChangeKind;
use mailcraft_model::{Contact, Id, Mailbox, Origin};
use serde_json::{Value, json};

use super::contacts_remote::{called, home, note_contact, server_copy};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session};

/// Most contacts one move takes.
pub const MAX_MOVE: usize = 1_000;
/// Of each contact, the addresses the duplicate check looks at.
const ADDRESSES_EACH: usize = 50;
/// Most contacts named when some can't move yet.
pub const NAMED: usize = 5;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("people.moveToFolder", "Move to Contacts Folder", "Home › Actions", move_to_folder)
        .params(r#"{"ids"?: [number] (1 to 1000 contacts; default: the selected one), "folder": number (a contacts folder's id, as people.get gives it, or 0 for On this computer)} — each contact goes into that folder: made on its server, and deleted from the server it leaves only once the new one is made there (until then it is in both places, never in neither); onto this computer at once, the server deleting its copy. A folder that may only be read, either way, refuses the whole move with the reason. Moves without asking about a contact the folder has already. A contact still being sent to its server isn't moved into a server's folder yet: it is left out and named, and when that is every one the move is refused. Answers {"moved": [{"from": id, "id": id now}], "folder", "skipped": [{"contact": id, "name"}]}"#)
        .selection(needs_contact)]
}

fn needs_contact(s: &Session) -> Option<&'static str> {
    if s.sel.contact.is_none() { Some("select a contact first") } else { None }
}

/// Where contact `c` is kept: its folder when a server keeps it, else 0 (On this computer).
fn place(mailbox: &Mailbox, c: &Contact) -> Id {
    if home(mailbox, c.folder).is_some() { c.folder } else { 0 }
}

/// The folder a move goes into: 0 (On this computer), or a folder of a server that may be written.
fn target(mailbox: &Mailbox, v: &Value) -> Result<Id, CmdError> {
    let id = v.get("folder").and_then(Value::as_u64).ok_or_else(|| bad("`folder` is a contacts folder's id, or 0 for On this computer"))?;
    if id == 0 {
        return Ok(0);
    }
    let f = home(mailbox, id).ok_or_else(|| failed!("no contacts folder with id {id}", id))?;
    if !f.writable {
        return Err(failed!("The contacts folder \"{name}\" can only be read: nothing can be moved into it.", name = f.name));
    }
    Ok(id)
}

/// The contacts named by `ids` (each once, in order), else the selected one; every one there and
/// none in a folder that may only be read. Those that can't move yet ([`waits`]) are left out, as
/// (id, name as [`called`] gives it); when that is all of them, the move is refused.
fn sources(s: &Session, v: &Value, to: Id) -> Result<(Vec<Id>, Vec<(Id, String)>), CmdError> {
    let wrong = || bad(format!("`ids` is a list of 1 to {MAX_MOVE} contact ids"));
    let named: Vec<Id> = match v.get("ids") {
        None => vec![s.sel.contact.ok_or_else(|| fail("select a contact first"))?],
        Some(list) => {
            let list = list.as_array().filter(|a| !a.is_empty() && a.len() <= MAX_MOVE).ok_or_else(wrong)?;
            list.iter().map(|x| x.as_u64().filter(|id| *id != 0).ok_or_else(wrong)).collect::<Result<_, _>>()?
        }
    };
    let mut seen = HashSet::new();
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for id in named.into_iter().filter(|id| seen.insert(*id)) {
        let c = s.mailbox.contact(id).ok_or_else(|| failed!("no contact with id {id}", id))?;
        if let Some(f) = home(&s.mailbox, c.folder).filter(|f| !f.writable) {
            return Err(failed!("The contacts folder \"{name}\" can only be read: its contacts can't be moved out of it.", name = f.name));
        }
        if waits(&s.mailbox, id, to) {
            skipped.push((id, called(Some(c))));
        } else {
            out.push(id);
        }
    }
    if !out.is_empty() || skipped.is_empty() {
        return Ok((out, skipped));
    }
    let names = skipped.iter().take(NAMED).map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ");
    Err(match skipped.len() {
        1 => failed!("{name} is still being sent to its server: try again shortly.", name = names),
        n if n <= NAMED => failed!("{names} are still being sent to their servers: try again shortly.", names),
        count => failed!("{count} contacts are still being sent to their servers, among them {names}: try again shortly.", count, names),
    })
}

/// Whether contact `id` can't go into folder `to` yet: it is still to be made on a server, or on
/// its way there unanswered (the sync thread may have sent it). Into a server's folder that could
/// leave it in neither place, or link one server's answer to another's folder: it moves once the
/// server has answered. Onto this computer it moves at once, under a new id ([`move_one`]), unless
/// an answer said it was tried (`sent`): then it waits for the answer or download that settles it,
/// as a record kept for a contact that is gone could be taken for another one made there later.
fn waits(mailbox: &Mailbox, id: Id, to: Id) -> bool {
    mailbox.contact_changes.iter().any(|n| n.contact == id && n.kind == ContactChangeKind::Create && (to != 0 || n.sent))
}

/// Move contact `id` into folder `to` (see the module): the id it has there; `None` when it is
/// there already.
fn move_one(mailbox: &mut Mailbox, id: Id, to: Id) -> Result<Option<Id>, CmdError> {
    let c = mailbox.contact(id).cloned().ok_or_else(|| failed!("no contact with id {id}", id))?;
    if place(mailbox, &c) == to {
        return Ok(None);
    }
    let account = home(mailbox, to).map_or(0, |f| f.account);
    let server = server_copy(mailbox, id);
    // One still to be made on a server, never tried ([`waits`]), may be on its way there: it
    // leaves its id behind and its note goes, so that a late answer that it was made is about that
    // id alone and has the server delete it again
    // ([`super::contacts_remote::apply_contact_outcomes`]), never taken for where this one goes.
    let creating = mailbox.contact_changes.iter().any(|n| n.contact == id && n.kind == ContactChangeKind::Create);
    if server.is_none() && !creating {
        // No server has it: it only changes folder (a new one there is noted to be made).
        let c = mailbox.contact_mut(id)?;
        (c.folder, c.origin, c.moved_from) = (to, Origin { account, ..Default::default() }, 0);
        note_contact(mailbox, id, None)?;
        return Ok(Some(id));
    }
    // A new contact in the target; this one goes from its server, once that one is made.
    let moved = Contact { id: 0, folder: to, origin: Origin { account, ..Default::default() }, moved_from: if to == 0 { 0 } else { id }, ..c };
    let new = mailbox.add_contact(moved);
    mailbox.remove_contact(id)?;
    note_contact(mailbox, id, server)?;
    note_contact(mailbox, new, None)?;
    Ok(Some(new))
}

fn move_to_folder(s: &mut Session, v: &Value) -> CmdResult {
    let to = target(&s.mailbox, v)?;
    let (ids, skipped) = sources(s, v, to)?;
    let mut moved = Vec::new();
    for id in ids {
        let Some(now) = move_one(&mut s.mailbox, id, to)? else { continue };
        moved.push(json!({"from": id, "id": now}));
        if s.sel.contact == Some(id) {
            s.sel.contact = Some(now);
        }
    }
    let skipped: Vec<Value> = skipped.into_iter().map(|(id, name)| json!({"contact": id, "name": name})).collect();
    Ok(json!({"moved": moved, "folder": to, "skipped": skipped}))
}

/// The contacts whose deletion from a server waits for a move: those a contact still to be made
/// on a server was moved from.
pub(crate) fn held_deletions(mailbox: &Mailbox) -> HashSet<Id> {
    mailbox
        .contact_changes
        .iter()
        .filter(|n| n.kind == ContactChangeKind::Create)
        .filter_map(|n| mailbox.contact(n.contact))
        .map(|c| c.moved_from)
        .filter(|from| *from != 0)
        .collect()
}

/// The server refused to make contact `id` (its note is gone): when it was moved from a server's
/// folder whose deletion still waits, it goes back there instead, as that server's copy, with what
/// the owner changed since going up as a change. The name of the folder it is back in.
pub(crate) fn moved_back(mailbox: &mut Mailbox, id: Id) -> Option<String> {
    let from = mailbox.contact(id).map(|c| c.moved_from).filter(|f| *f != 0)?;
    let at = mailbox.contact_changes.iter().position(|n| n.contact == from && n.kind == ContactChangeKind::Delete && n.base.is_some())?;
    let note = mailbox.contact_changes.remove(at);
    let base = note.base?;
    let name = mailbox.contact_folder(note.folder).map(|f| f.name.clone()).unwrap_or_default();
    let c = mailbox.contact_mut(id).ok()?;
    (c.folder, c.origin, c.moved_from) = (note.folder, base.origin.clone(), 0);
    // (Two notes went: there is room for this one.)
    let _ = note_contact(mailbox, id, Some(*base));
    Some(name)
}

/// For the window's dialog only: a contact in folder `folder` (0: On this computer) with one of
/// the e-mail addresses of the contacts `ids` (compared trimmed, without telling capitals from
/// small letters), as (the one being moved, the one there). A programmatic move doesn't ask.
pub fn already_in(mailbox: &Mailbox, ids: &[Id], folder: Id) -> Option<(Id, Id)> {
    let key = |a: &str| a.trim().to_lowercase();
    let mut wanted: HashMap<String, Id> = HashMap::new();
    for c in ids.iter().take(MAX_MOVE).filter_map(|id| mailbox.contact(*id)).filter(|c| place(mailbox, c) != folder) {
        for e in c.emails.iter().take(ADDRESSES_EACH).map(|e| key(&e.value)).filter(|e| !e.is_empty()) {
            wanted.entry(e).or_insert(c.id);
        }
    }
    if wanted.is_empty() {
        return None;
    }
    let moving: HashSet<Id> = ids.iter().copied().collect();
    mailbox
        .contacts
        .values()
        .filter(|c| !moving.contains(&c.id) && place(mailbox, c) == folder)
        .find_map(|there| there.emails.iter().take(ADDRESSES_EACH).find_map(|e| wanted.get(&key(&e.value)).map(|mine| (*mine, there.id))))
}

#[cfg(test)]
#[path = "contacts_move_tests.rs"]
mod contacts_move_tests;
