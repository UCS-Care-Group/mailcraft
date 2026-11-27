// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts kept in step with an account's server, both ways (#1917).
//!
//! The engine does no I/O: a host brings a [`ContactsDownload`] of one contacts folder
//! (`sync.applyContacts`, [`merge_contacts`]), asks what is to go up (`sync.contactChanges`,
//! [`contact_upload`]) and says what the server made of it (`sync.applyContactChanges`,
//! [`apply_contact_outcomes`]).
//!
//! **One note a contact.** A command that makes, changes or deletes a contact of a writable
//! folder makes the change here and then calls [`note_contact`] with what the server has of the
//! contact ([`server_copy`], read before the change). The note is worked out afresh each time,
//! from the server's copy and the contact as it is now: none → there is to make it, both → the
//! fields that differ, the server's only → to delete. So two changes before the server hears of
//! either are one, a change taken back is none, and an undo is just another such change
//! ([`after_undo`]): undone after it went up, the server is told the opposite.
//!
//! **Changed in both places.** A download merges field by field: the fields the owner changed
//! stay as they are here (and still go up), every other field is the server's. A contact the
//! server deleted that was changed here is made again; one deleted here and still listed there is
//! left out until the deletion is through.
//!
//! **Refused for good.** The contact goes back to the server's copy (one deleted comes back, one
//! made is kept On this computer) and the owner is told once. What fails for now waits, longer
//! after each failure, and is in the mailbox file, so it outlives a restart.
//!
//! What only this computer keeps never goes up: whether a contact is a favourite, its keys,
//! where it was imported from ([`ContactField`] names what does).

use std::collections::{BTreeSet, HashMap, HashSet};

use mailcraft_model::contact_sync::{
    ContactChange, ContactChangeKind, ContactField, ContactFolder, ContactOp, ContactResult, ContactsUpload, ContactsUploaded, LONGEST_WAIT,
    MAX_CONTACT_CHANGES, MAX_CONTACT_OPS, RemoteContactFolder, delta, wait_after,
};
use mailcraft_model::remote::{ContactsDownload, MAX_CONTACTS, RemoteContact, clean_key};
use mailcraft_model::sync::JmapContactPlace;
use mailcraft_model::{AccountChoice, Contact, Id, Mailbox, Origin};
use serde_json::{Value, json};

use super::remote::{Merged, contact_from_remote, owner};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Most outcomes taken from one answer, and most lines said about them.
const MAX_OUTCOMES: usize = 1_000;
const MAX_MESSAGES: usize = 20;
/// Longest text of a parameter error handed back.
const MAX_SHOWN: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.contactChanges", "Contact Changes to Send", "Send / Receive", contact_changes_command)
            .pure()
            .params(r#"{"account": number} — what that account's server is to be told now of contacts made, changed or deleted here, oldest first: {"account", "email", "ops": [{"kind": "create"|"change"|"delete", "contact": number, "remote"?: string, "folder"?: string (the server's folder; none: the main one), "owner"?: {"mailbox": address} | {"person": address} (none: the account's own), "fields"?: ["displayName"|"first"|"middle"|"last"|"nickname"|"company"|"title"|"department"|"emails"|"phones"|"addresses"|"websites"|"birthday"|"notes"|"categories"] (only these are written), "values": RemoteContact}]} — changes of a folder that may only be read now, and a new contact the server said it made without saying its id, are held"#),
        CommandSpec::new("sync.applyContactChanges", "Apply Sent Contact Changes", "Send / Receive", apply_contact_changes_command)
            .pure()
            .params(r#"{"result": ContactsUploaded} — what a host heard from the server: {"account": number, "email"?: string, "outcomes": [{"op": ContactOp, "result": "done"|"gone"|"refused"|"denied"|"failed", "remote"?: string, "changeKey"?: string, "status"?: number, "error"?: string, "retryAfter"?: seconds}], "tokenRefused"?: bool} — answers {"done", "refused", "failed", "denied", "waiting", "messages": [string]}"#),
    ]
}

/// The contacts folder `folder` when a server keeps it: its account is in the mailbox.
pub fn home(mailbox: &Mailbox, folder: Id) -> Option<&ContactFolder> {
    mailbox.contact_folder(folder).filter(|f| mailbox.account(f.account).is_some())
}

/// Whether the contact is in a folder the account may only read: it can't be changed or deleted
/// here ([`changeable`] says why).
pub fn read_only(mailbox: &Mailbox, c: &Contact) -> bool {
    home(mailbox, c.folder).is_some_and(|f| !f.writable)
}

/// Refuse to change or delete a contact of a folder the account may only read.
pub(crate) fn changeable(mailbox: &Mailbox, c: &Contact) -> Result<(), CmdError> {
    match home(mailbox, c.folder) {
        Some(f) if !f.writable => {
            Err(failed!("The contacts folder \"{name}\" can only be read: this contact can't be changed or deleted here.", name = f.name))
        }
        _ => Ok(()),
    }
}

/// Whether a change to the contact waits to go up ("Not sent yet").
pub fn not_sent(mailbox: &Mailbox, contact: Id) -> bool {
    mailbox.contact_changes.iter().any(|c| c.contact == contact)
}

/// What the server has of contact `id`, as far as `mailbox` knows: the copy a waiting change
/// started from, else the contact itself when it is one of a server's; `None` when the server
/// has none (it was made here and not sent yet, or it is this computer's).
pub(crate) fn server_copy(mailbox: &Mailbox, id: Id) -> Option<Contact> {
    match mailbox.contact_changes.iter().find(|c| c.contact == id) {
        Some(c) if c.kind == ContactChangeKind::Create => None,
        Some(c) => c.base.as_deref().cloned().or_else(|| mailbox.contact(id).cloned()),
        None => mailbox.contact(id).filter(|c| !c.origin.id.is_empty() && home(mailbox, c.folder).is_some()).cloned(),
    }
}

/// Note what the server of contact `id` is to be told, now that the contact is as it is here,
/// when the server has `server` of it ([`server_copy`] from before the change). The contact's
/// own idea of what the server calls it is set to match. Refused, with a reason, when a new note
/// would be one too many ([`MAX_CONTACT_CHANGES`]); nothing is changed then.
pub(crate) fn note_contact(mailbox: &mut Mailbox, id: Id, server: Option<Contact>) -> Result<(), CmdError> {
    let server = server.filter(|s| !s.origin.id.is_empty());
    let at = mailbox.contact_changes.iter().position(|c| c.contact == id);
    let here = mailbox.contact(id).cloned();
    // A new contact that was sent, and is gone here (undone, deleted) before its server's name for
    // it was heard: its note stays, as the record that it is not wanted. When a download shows it
    // there, it is deleted there ([`merge_contacts`]); it is never sent again.
    if here.is_none()
        && server.is_none()
        && at.and_then(|at| mailbox.contact_changes.get(at)).is_some_and(|c| c.kind == ContactChangeKind::Create && c.sent)
    {
        return Ok(());
    }
    // Where it is here: on a server that may change it, on one that may not (nothing to tell),
    // or on this computer (or nowhere). A change that waits when its folder becomes one that may
    // only be read is kept (and held: [`contact_upload`] doesn't send it), not lost.
    let held = at.is_some();
    let place = here.as_ref().and_then(|c| home(mailbox, c.folder)).map(|f| (f.id, f.account, f.writable || held));
    let (change, origin) = match (&here, place, server) {
        (Some(_), Some((_, _, false)), _) => (None, None),
        (Some(_), Some((folder, account, true)), None) => {
            let change = ContactChange { account, kind: ContactChangeKind::Create, contact: id, folder, ..Default::default() };
            (Some(change), Some(Origin { account, ..Default::default() }))
        }
        (Some(c), Some((folder, account, true)), Some(s)) => {
            let origin = Origin { account, id: s.origin.id.clone(), change_key: s.origin.change_key.clone(), ..Default::default() };
            let fields = ContactField::changed(&s, c);
            // Nothing differs from the server's copy: nothing to tell.
            let change = (!fields.is_empty()).then(|| ContactChange {
                account,
                kind: ContactChangeKind::Change,
                contact: id,
                folder,
                remote: s.origin.id.clone(),
                fields,
                base: Some(Box::new(s)),
                ..Default::default()
            });
            (change, Some(origin))
        }
        // Gone from here, or kept on this computer now: the server is to delete its copy, when
        // its account is still here.
        (_, None, Some(s)) if home(mailbox, s.folder).is_some() => {
            let change = ContactChange {
                account: s.origin.account,
                kind: ContactChangeKind::Delete,
                contact: id,
                folder: s.folder,
                remote: s.origin.id.clone(),
                base: Some(Box::new(s)),
                ..Default::default()
            };
            (Some(change), None)
        }
        _ => (None, None),
    };
    put_note(mailbox, at, change)?;
    if let Ok(c) = mailbox.contact_mut(id) {
        match origin {
            Some(origin) => c.origin = origin,
            // One kept on this computer has no server.
            None if place.is_none() && c.origin.is_remote() => c.origin = Origin::default(),
            None => {}
        }
    }
    Ok(())
}

/// Put `change` where the note at `at` was (or none).
fn put_note(mailbox: &mut Mailbox, at: Option<usize>, change: Option<ContactChange>) -> Result<(), CmdError> {
    match (at, change) {
        (Some(at), Some(mut change)) => {
            if let Some(old) = mailbox.contact_changes.get_mut(at) {
                // Something new to say: it doesn't wait out what the last try earned, but a
                // refusal of the permission once is remembered, and so is whether it was sent
                // (and, for a new one, what was sent and whether the server made it already).
                if old.kind == change.kind {
                    (change.attempts, change.denied, change.sent) = (old.attempts, old.denied, old.sent);
                    if change.kind == ContactChangeKind::Create {
                        (change.unconfirmed, change.tried) = (old.unconfirmed, old.tried);
                        change.base = old.base.take();
                    }
                }
                *old = change;
            }
        }
        (Some(at), None) => {
            mailbox.contact_changes.remove(at);
        }
        (None, Some(change)) => {
            if mailbox.contact_changes.len() >= MAX_CONTACT_CHANGES {
                return Err(fail("Too many contact changes are waiting to be sent to the server. Let Send/Receive get them through first."));
            }
            mailbox.contact_changes.push(change);
        }
        (None, None) => {}
    }
    Ok(())
}

/// After an undo or a redo (`mailbox`, from the mailbox that was `now`): the contacts the step
/// touched are as the step had them, and what their servers are told is worked out again from
/// what the servers have now (so an undo after a change went up sends the opposite). Every other
/// contact's note, and the folders, are as `now` has them.
///
/// A contact whose change can't be noted for want of room, or that is (or was) in a folder that
/// may only be read now with nothing waiting for it, isn't undone: it stays as `now` has it, so
/// nothing is undone here that its server isn't told of (a deletion included). How many weren't,
/// for want of room and for a folder that may only be read.
pub(crate) fn after_undo(mailbox: &mut Mailbox, now: &Mailbox, touched: &crate::undo::Touched) -> (usize, usize) {
    mailbox.contact_folders.clone_from(&now.contact_folders);
    // (The touched contacts' notes stay where they are and are worked out again in place, so
    // what was sent of them is remembered.)
    mailbox.contact_changes.clone_from(&now.contact_changes);
    let (mut no_room, mut read_only_kept) = (0, 0);
    let keep = |mailbox: &mut Mailbox, id: Id| match now.contacts.get(&id) {
        Some(c) => {
            mailbox.contacts.insert(id, c.clone());
        }
        None => {
            mailbox.contacts.remove(&id);
        }
    };
    for id in touched.changed_contacts() {
        // Of a contact the step changed, only the fields it changed go back: what a server
        // changed since in the others, and the server's key for it, stay as they are now.
        if let (Some(fields), Some(cur)) = (touched.contact_fields(*id), now.contact(*id))
            && let Ok(c) = mailbox.contact_mut(*id)
        {
            for field in ContactField::ALL.into_iter().filter(|f| !fields.contains(f)) {
                field.copy(cur, c);
            }
            // One the step moved goes back where it was (no server had it:
            // [`super::contacts_move`]). Any other stays where it is now, also when a refusal or
            // a move back put it elsewhere since, outside any step.
            if !touched.moved_contact(*id) {
                (c.origin, c.folder, c.moved_from) = (cur.origin.clone(), cur.folder, cur.moved_from);
            }
        }
        // In a folder that may only be read now, with nothing waiting for it (which would be
        // held, as any change there is): its server can't be told, so it stays as it is now.
        let shut = |m: &Mailbox| m.contact(*id).is_some_and(|c| read_only(m, c));
        let differs = match (mailbox.contact(*id), now.contact(*id)) {
            (Some(a), Some(b)) => !ContactField::changed(b, a).is_empty(),
            (a, b) => a.is_some() != b.is_some(),
        };
        if differs && (shut(mailbox) || shut(now)) && !not_sent(now, *id) {
            keep(mailbox, *id);
            read_only_kept += 1;
        } else if note_contact(mailbox, *id, server_copy(now, *id)).is_err() {
            // No room to tell its server: this contact stays as it is now (with what waits for
            // it already), rather than undone here and not there.
            keep(mailbox, *id);
            no_room += 1;
        }
    }
    // A contact a move took away, back and to be made on its server again: the one the move made,
    // taken away now, is deleted from its server only once that is done, as the move held its own
    // deletion (#1144). So too when its deletion was waiting, and so may be on its way: an answer
    // that it was deleted makes it again ([`apply_contact_outcomes`]), and the link holds this one's
    // deletion till then.
    // lean: this holds once the answer is in. Before it, the hold rests on the jobs' turns
    // (`sync_driver::jobs::Busy`): a Graph calendar job doesn't start while an IMAP, Exchange or
    // JMAP run is at work, but a JMAP run can start while a calendar job is in flight. So with the
    // source's deletion on its way to Microsoft, a JMAP account's run can send this one's deletion
    // first; should the source then not be made again, it is kept on this computer only (a refused
    // Create keeps it there: [`give_up`]), never lost. Upgrade: hold this one's deletion until the
    // source's account has answered once after the undo.
    for id in touched.changed_contacts() {
        let Some(from) = now.contact(*id).filter(|_| mailbox.contact(*id).is_none()).map(|c| c.moved_from).filter(|f| *f != 0) else { continue };
        let creating = mailbox.contact_changes.iter().any(|n| n.contact == from && n.kind == ContactChangeKind::Create)
            || now.contact_changes.iter().any(|n| n.contact == from && n.kind == ContactChangeKind::Delete);
        if creating && let Ok(c) = mailbox.contact_mut(from) {
            c.moved_from = *id;
        }
    }
    (no_room, read_only_kept)
}

/// The folder a new contact goes into when the command doesn't say (C8), without making
/// anything: the folder the list shows alone, when a contact can go there; else with one account
/// that keeps contacts, its main folder; with more, the account the list shows (On this computer
/// when it shows that), else the folder last used, else the first account's. `Err(account)` is
/// that account's main folder, not made yet.
pub fn default_contact_folder(s: &Session) -> Result<Id, Id> {
    let usable = |f: Id| f == 0 || home(&s.mailbox, f).is_some_and(|f| f.writable);
    if let Some(folder) = super::people::shown_folder(s).filter(|f| usable(*f)) {
        return Ok(folder);
    }
    let accounts = super::people::contact_accounts(&s.mailbox);
    let account = match accounts.as_slice() {
        [] => return Ok(0),
        [one] => *one,
        [first, ..] => match super::people::shown_account(s) {
            AccountChoice::Account(a) => a,
            AccountChoice::Here => return Ok(0),
            AccountChoice::All => match s.mailbox.contact_place.filter(|f| usable(*f)) {
                Some(place) => return Ok(place),
                None => *first,
            },
        },
    };
    match s.mailbox.main_contact_folder(account) {
        Some(main) => Ok(if usable(main) { main } else { 0 }),
        None => Err(account),
    }
}

/// The folder a new contact goes into: `"folder"` says it (0: On this computer; one that may
/// only be read is refused), else [`default_contact_folder`], made when it isn't yet.
pub(crate) fn new_contact_folder(s: &mut Session, v: &Value) -> Result<Id, CmdError> {
    if v.get("folder").is_some_and(|f| !f.is_null()) {
        let id = p::u64(v, "folder").ok_or_else(|| bad("`folder` is a contacts folder's id, or 0 for On this computer"))?;
        if id == 0 {
            return Ok(0);
        }
        let f = home(&s.mailbox, id).ok_or_else(|| failed!("no contacts folder with id {id}", id))?;
        if !f.writable {
            return Err(failed!("The contacts folder \"{name}\" can only be read: a new contact can't be put there.", name = f.name));
        }
        return Ok(id);
    }
    match default_contact_folder(s) {
        Ok(folder) => Ok(folder),
        Err(account) => s.mailbox.ensure_main_contact_folder(account).ok_or_else(|| fail("this account has too many contacts folders")),
    }
}

/// What only this computer keeps of a contact stays as `old` has it when the server's copy comes
/// in: its id, whether it is a favourite, its keys, where it was imported from, its vCard UID,
/// and its categories when the server didn't say.
fn keep_local(fresh: &mut Contact, old: &Contact, categories: bool) {
    fresh.id = old.id;
    fresh.favorite = old.favorite;
    fresh.keys.clone_from(&old.keys);
    fresh.uid.clone_from(&old.uid);
    fresh.imported.clone_from(&old.imported);
    fresh.imported_from.clone_from(&old.imported_from);
    fresh.moved_from = old.moved_from;
    if categories {
        fresh.categories.clone_from(&old.categories);
    }
}

/// The folder a download is of, made or brought up to date here, and whether that changed the
/// mailbox. Nothing is changed when this fails.
fn take_folder(mailbox: &mut Mailbox, account: Id, listed: Option<&RemoteContactFolder>) -> Result<(Id, bool), CmdError> {
    let too_many = || fail("this account has too many contacts folders");
    let Some(listed) = listed else {
        let had = mailbox.main_contact_folder(account).is_some();
        return Ok((mailbox.ensure_main_contact_folder(account).ok_or_else(too_many)?, !had));
    };
    let folder = ContactFolder {
        account,
        remote: listed.id.clone(),
        name: listed.name.clone(),
        owner: listed.owner.clone(),
        writable: listed.writable,
        ..Default::default()
    }
    .tidied()
    .ok_or_else(|| bad("the contacts folder's id can't be read, or its owner isn't an e-mail address"))?;
    let parent = clean_key(&listed.parent);
    let parent = if parent.is_empty() { 0 } else { mailbox.contact_folder_named(account, &folder.owner, &parent).unwrap_or(0) };
    match mailbox.contact_folder_named(account, &folder.owner, &folder.remote) {
        Some(id) => {
            let mut changed = false;
            if let Some(f) = mailbox.contact_folders.iter_mut().find(|f| f.id == id)
                && (f.name != folder.name || f.writable != folder.writable || f.parent != parent)
            {
                (f.name, f.writable, f.parent) = (folder.name, folder.writable, parent);
                changed = true;
            }
            Ok((id, changed))
        }
        None => Ok((mailbox.add_contact_folder(ContactFolder { parent, ..folder }).ok_or_else(too_many)?, true)),
    }
}

/// Two contacts are the same person for taking in one made here whose answer was lost.
fn same_person(a: &Contact, b: &Contact) -> bool {
    a.name() == b.name() && a.primary_email() == b.primary_email()
}

/// A new contact here that was sent and whose answer may have been lost, matching `fresh`:
/// matched against what was sent (the owner may have changed it since).
fn twin(mailbox: &Mailbox, account: Id, folder: Id, fresh: &Contact) -> Option<Id> {
    mailbox
        .contact_changes
        .iter()
        .filter(|c| c.account == account && c.kind == ContactChangeKind::Create && c.folder == folder && c.sent)
        .filter(|c| c.base.as_deref().or_else(|| mailbox.contact(c.contact)).is_some_and(|sent| same_person(sent, fresh)))
        // One still here first: rather taken in than deleted there when it can't be told which.
        .min_by_key(|c| mailbox.contact(c.contact).is_none())
        .map(|c| c.contact)
}

/// The retry state of a note: kept when a download leaves what it says as it was.
fn retry_state(change: &ContactChange) -> (u32, i64, bool, String) {
    (change.attempts, change.next_try, change.denied, change.error.clone())
}

/// Merge one contacts folder's contacts as a server has them into the mailbox (see the module).
///
/// - A contact the account's server calls by an id the mailbox has is brought up to date (field
///   by field when a change to it waits); one it doesn't have is added, or is a contact made here
///   whose answer was lost.
/// - A contact the download says is gone (`gone`, or not listed in a complete download of its
///   folder) goes, unless it was changed here: then it is made again.
/// - Contacts of other folders, of other accounts and of this computer are not looked at. Doing
///   it twice changes nothing the second time. Nothing is changed when this returns an error.
pub fn merge_contacts(mailbox: &mut Mailbox, download: &ContactsDownload) -> Result<Merged, CmdError> {
    let owner = owner(mailbox, download.account, &download.email)?;
    // (Microsoft Graph keeps only so many of a contact's addresses and numbers.)
    let (account, graph) = (owner.id, owner.contacts_in_graph());
    let (folder, folder_changed) = take_folder(mailbox, account, download.folder.as_ref())?;
    let mut merged = Merged { checked: folder_changed, ..Merged::default() };
    let queue = mailbox.contact_changes.clone();
    // (More than may be taken in: only a part is, so the download is no whole list, and where its
    // list of changes was read up to isn't kept.)
    let whole = download.contacts.len() <= MAX_CONTACTS;
    let mut known: HashMap<String, Id> = HashMap::new();
    let mut extra: Vec<Id> = Vec::new();
    // (Only this folder's: what another folder of the account lists, or says is gone, is not this one's.)
    for c in mailbox.contacts.values().filter(|c| c.origin.account == account && !c.origin.id.is_empty() && c.folder == folder) {
        extra.extend(known.insert(c.origin.id.clone(), c.id));
    }
    // Deleted here and not told yet: still listed there, and not brought back.
    let deleting: HashSet<String> =
        mailbox.contact_changes.iter().filter(|c| c.folder == folder && c.kind == ContactChangeKind::Delete).map(|c| c.remote.clone()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    for remote in download.contacts.iter().take(MAX_CONTACTS) {
        let mut remote = remote.clone();
        remote.tidy();
        let mut fresh = contact_from_remote(&remote);
        // Somebody with no name, company or address would be an empty line in the list.
        if remote.id.is_empty() || fresh.name().is_empty() || !seen.insert(remote.id.clone()) {
            merged.skipped += 1;
            continue;
        }
        if deleting.contains(&remote.id) {
            merged.unchanged += 1;
            continue;
        }
        fresh.origin.account = account;
        fresh.folder = folder;
        match known.get(&remote.id).and_then(|id| mailbox.contact(*id)).cloned() {
            Some(old) => {
                let waiting = mailbox.contact_changes.iter().find(|c| c.contact == old.id);
                let mine = waiting.map(|c| c.fields.clone());
                // The server's copy: a server that didn't say what its categories are has those
                // it had.
                let had = waiting.and_then(|c| c.base.as_deref()).map_or(&old.categories, |base| &base.categories).clone();
                keep_local(&mut fresh, &old, false);
                // What Graph has no place for (a fourth address, another mobile number) stays here.
                // (The server's copy then has those that still have no place there: one that has
                // a place now is a change to send.)
                let mut server = graph.then(|| mailcraft_model::contact_sync::keep_graph_overflow(&mut fresh, &old));
                if remote.categories.is_none() {
                    fresh.categories.clone_from(&had);
                    if let Some(server) = server.as_mut() {
                        server.categories = had;
                    }
                }
                let server = server.unwrap_or_else(|| fresh.clone());
                let new = match &mine {
                    Some(fields) => ContactField::merged(&old, &fresh, fields),
                    None => fresh.clone(),
                };
                if new == old {
                    merged.unchanged += 1;
                } else if let Ok(c) = mailbox.contact_mut(old.id) {
                    *c = new;
                    merged.updated += 1;
                }
                if mine.is_none() && server != fresh {
                    // (No room for it: it stays here, and is sent with the next change.)
                    let _ = note_contact(mailbox, old.id, Some(server));
                } else if mine.is_some() {
                    let before = mailbox.contact_changes.iter().find(|c| c.contact == old.id).cloned();
                    // (The note is there already: there is room for it.)
                    let _ = note_contact(mailbox, old.id, Some(server));
                    // The same note against the same copy of the server's: it waits out what its
                    // last try earned.
                    if let Some(before) = before
                        && let Some(after) = mailbox.contact_changes.iter_mut().find(|c| c.contact == old.id)
                        && (after.kind, &after.fields, &after.remote) == (before.kind, &before.fields, &before.remote)
                        && let (Some(a), Some(b)) = (after.base.as_deref(), before.base.as_deref())
                        && ContactField::changed(a, b).is_empty()
                        && a.origin.change_key == b.origin.change_key
                    {
                        (after.attempts, after.next_try, after.denied, after.error) = retry_state(&before);
                    }
                }
            }
            None => match twin(mailbox, account, folder, &fresh).inspect(|twin| {
                // A server that didn't say what its categories are has those that were sent.
                if remote.categories.is_none() {
                    let sent = mailbox
                        .contact_changes
                        .iter()
                        .find(|c| c.contact == *twin)
                        .and_then(|c| c.base.as_deref())
                        .or_else(|| mailbox.contact(*twin));
                    fresh.categories = sent.map(|c| c.categories.clone()).unwrap_or_default();
                }
            }) {
                // Made there, and no longer wanted here: it is deleted there.
                Some(twin) if mailbox.contact(twin).is_none() => {
                    if let Some(note) = mailbox.contact_changes.iter_mut().find(|c| c.contact == twin) {
                        *note = ContactChange {
                            account,
                            kind: ContactChangeKind::Delete,
                            contact: twin,
                            folder,
                            remote: remote.id.clone(),
                            base: Some(Box::new(fresh)),
                            ..Default::default()
                        };
                    }
                    merged.skipped += 1;
                }
                Some(twin) => {
                    let server = match mailbox.contact(twin).cloned() {
                        Some(here) if graph => mailcraft_model::contact_sync::keep_graph_overflow(&mut fresh, &here),
                        _ => fresh,
                    };
                    let _ = note_contact(mailbox, twin, Some(server));
                    merged.updated += 1;
                }
                None => {
                    mailbox.add_contact(fresh);
                    merged.added += 1;
                }
            },
        }
    }
    // What the server no longer has.
    let mut gone: HashSet<String> = download.gone.iter().map(|id| clean_key(id)).filter(|id| !id.is_empty() && !seen.contains(id)).collect();
    if download.complete && whole {
        // A new contact the server said it made without saying its name, and which a whole list
        // read after that answer (by this computer's clock) doesn't show: it wasn't made after
        // all, and goes up again. A list not said to be read after it says nothing about it.
        let read = download.read_at;
        for c in mailbox
            .contact_changes
            .iter_mut()
            .filter(|c| c.folder == folder && c.kind == ContactChangeKind::Create && c.unconfirmed != 0 && read > c.unconfirmed)
        {
            c.unconfirmed = 0;
        }
        // One no longer wanted here that such a list, read after it was last tried, doesn't show
        // either: nothing left to do.
        let contacts = &mailbox.contacts;
        mailbox.contact_changes.retain(|c| {
            let unwanted = c.folder == folder && c.kind == ContactChangeKind::Create && c.sent && !contacts.contains_key(&c.contact);
            !(unwanted && c.unconfirmed == 0 && read > c.tried)
        });
        let listed_here = |id: &Id| mailbox.contact(*id).is_some_and(|c| c.folder == folder);
        gone.extend(known.iter().filter(|(remote, id)| !seen.contains(*remote) && listed_here(id)).map(|(remote, _)| remote.clone()));
        gone.extend(mailbox.contact_changes.iter().filter(|c| c.folder == folder && c.kind == ContactChangeKind::Delete).map(|c| c.remote.clone()));
        gone.retain(|remote| !seen.contains(remote));
        // (A second contact here under an id the server has once.)
        let twice: Vec<Id> = extra.into_iter().filter(|id| listed_here(id)).collect();
        for id in twice {
            if mailbox.remove_contact(id).is_ok() {
                merged.removed += 1;
            }
        }
    }
    for remote in gone {
        // A deletion made here that the server has had (or another device made).
        mailbox.contact_changes.retain(|c| !(c.folder == folder && c.kind == ContactChangeKind::Delete && c.remote == remote));
        let Some(id) = known.get(&remote).copied() else { continue };
        if not_sent(mailbox, id) {
            // Changed here: made again rather than lost.
            if let Ok(c) = mailbox.contact_mut(id) {
                c.origin.id.clear();
                c.origin.change_key.clear();
            }
            let _ = note_contact(mailbox, id, None);
            merged.updated += 1;
        } else if mailbox.remove_contact(id).is_ok() {
            merged.removed += 1;
        }
    }
    let position = delta(&download.delta);
    // (A whole list with no position, a folder the server has nothing in, drops one it no longer
    // has: the next read is whole again, not from a position it forgot.)
    if whole
        && (!position.is_empty() || download.complete)
        && let Some(f) = mailbox.contact_folders.iter_mut().find(|f| f.id == folder)
        && f.delta != position
    {
        f.delta = position;
        merged.checked = true;
    }
    merged.checked |= mailbox.contact_changes != queue;
    Ok(merged)
}

/// What `account`'s server is to be told now: the changes that are due, oldest first,
/// [`MAX_CONTACT_OPS`] at most. Those of a folder the account may only read now are held, and so
/// is a new contact the server said it made without saying its name. Nothing is changed: a
/// change counts as sent only once an answer says it was tried ([`apply_contact_outcomes`]).
pub fn contact_upload(s: &Session, account: Id) -> Result<ContactsUpload, CmdError> {
    let owner = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    let (mailbox, now) = (&s.mailbox, s.now());
    let mut ops = Vec::new();
    // A deletion a move left waits until the contact moved is made ([`super::contacts_move`]).
    let held = super::contacts_move::held_deletions(mailbox);
    for change in mailbox.contact_changes.iter().filter(|c| c.account == account && c.next_try <= now && c.unconfirmed == 0) {
        if ops.len() >= MAX_CONTACT_OPS {
            break;
        }
        if change.kind == ContactChangeKind::Delete && held.contains(&change.contact) {
            continue;
        }
        let Some(folder) = mailbox.contact_folder(change.folder).filter(|f| f.account == account && f.writable) else { continue };
        let contact = mailbox.contact(change.contact);
        let (values, fields) = match (change.kind, contact) {
            (ContactChangeKind::Create, Some(c)) => (RemoteContact::of(c), ContactField::ALL.into_iter().collect()),
            (ContactChangeKind::Change, Some(c)) if !change.fields.is_empty() && !change.remote.is_empty() => {
                let mut values = RemoteContact::of(c);
                values.change_key = change.base.as_deref().map(|b| b.origin.change_key.clone()).unwrap_or_default();
                (values, change.fields.clone())
            }
            (ContactChangeKind::Delete, _) if !change.remote.is_empty() => {
                let change_key = change.base.as_deref().map(|b| b.origin.change_key.clone()).unwrap_or_default();
                (RemoteContact { id: change.remote.clone(), change_key, ..Default::default() }, BTreeSet::new())
            }
            _ => continue,
        };
        ops.push(ContactOp {
            kind: change.kind,
            contact: change.contact,
            remote: change.remote.clone(),
            folder: folder.remote.clone(),
            owner: folder.owner.clone(),
            fields,
            values,
        });
    }
    Ok(ContactsUpload { ops, ..Default::default() }.for_account(owner))
}

/// The sign-in of `account` changed (the owner gave a permission): what was held back is tried
/// again at once, and a folder of somebody else's that refused a change once may be changed
/// again (the server says when it still may not: it can't be asked beforehand).
pub fn retry_contact_changes(s: &mut Session, account: Id) {
    let mut any = false;
    for change in s.mailbox.contact_changes.iter_mut().filter(|c| c.account == account && c.next_try != 0) {
        change.next_try = 0;
        any = true;
    }
    for folder in s.mailbox.contact_folders.iter_mut().filter(|f| f.account == account && !f.owner.is_own() && !f.writable) {
        folder.writable = true;
        any = true;
    }
    if any {
        s.mailbox_changed();
    }
}

/// Whether a change to `account`'s contacts is due to go up at `now`.
pub fn contacts_due(mailbox: &Mailbox, account: Id, now: mailcraft_time::Timestamp) -> bool {
    mailbox.contact_changes.iter().any(|c| c.account == account && c.next_try <= now)
}

/// The sign-in of `account` doesn't cover changing contacts: what waits stays here, says why
/// ("Not sent yet"), and isn't tried again before the owner allows it
/// ([`retry_contact_changes`]) or a long wait is over.
pub fn wait_for_permission(s: &mut Session, account: Id) {
    let why = said!("Waiting for your permission to change this account's contacts on its server: File › Info › Ask again").to_string();
    let later = s.now().saturating_add(LONGEST_WAIT);
    let mut any = false;
    for change in s.mailbox.contact_changes.iter_mut().filter(|c| c.account == account) {
        change.error.clone_from(&why);
        change.next_try = change.next_try.max(later);
        any = true;
    }
    if any {
        s.mailbox_changed();
    }
}

/// What taking in the answers to a [`ContactsUpload`] came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactsApplied {
    pub done: usize,
    /// Refused for good: put back as the server has them, and said.
    pub refused: usize,
    /// To be tried again later.
    pub failed: usize,
    /// The account may not change these contacts: its owner is to be asked for the permission.
    pub denied: bool,
    /// What still waits for the account, after this.
    pub waiting: usize,
    /// What the owner should hear, a line each.
    pub messages: Vec<String>,
}

impl ContactsApplied {
    fn say(&mut self, line: String) {
        if self.messages.len() < MAX_MESSAGES && !self.messages.contains(&line) {
            self.messages.push(line);
        }
    }

    pub fn json(&self) -> Value {
        json!({"done": self.done, "refused": self.refused, "failed": self.failed, "denied": self.denied, "waiting": self.waiting, "messages": self.messages})
    }
}

/// A name for a line about a contact: short, and never empty.
pub(crate) fn called(c: Option<&Contact>) -> String {
    let name: String = c.map(Contact::name).unwrap_or_default().chars().filter(|ch| !ch.is_control()).take(60).collect();
    if name.trim().is_empty() { "A contact".to_string() } else { format!("\"{}\"", name.trim()) }
}

/// Take in what a server said to a [`ContactsUpload`] (see the module). Not an undo step.
///
/// - Done: the server's copy is what was sent, and the note is worked out again against it (the
///   contact may have changed again meanwhile, or been undone: then the rest, or the opposite,
///   goes next).
/// - Gone: a deletion is done; a change makes the contact again.
/// - Refused (and Denied a second time): put back as the server has it, and said once.
/// - Denied the first time, and Failed: kept, and tried again later, longer after each failure.
pub fn apply_contact_outcomes(s: &mut Session, uploaded: &ContactsUploaded) -> Result<ContactsApplied, CmdError> {
    let account = owner(&s.mailbox, uploaded.account, &uploaded.email)?.id;
    let now = s.now();
    let mut applied = ContactsApplied::default();
    let mailbox = &mut s.mailbox;
    // Folders of somebody else's that refused a change: read-only once every answer is in, so
    // that what the owner changed since in any of them stays noted (and is then held).
    let mut refused: Vec<Id> = Vec::new();
    for outcome in uploaded.outcomes.iter().take(MAX_OUTCOMES) {
        let mut outcome = outcome.clone();
        outcome.tidy();
        let (id, kind) = (outcome.op.contact, outcome.op.kind);
        let at = mailbox.contact_changes.iter().position(|c| c.account == account && c.contact == id && c.kind == kind);
        // An answer about another of the server's contacts than the one this contact is linked to
        // now (it was made again meanwhile, or the answer names a different one) isn't about
        // this contact: left alone.
        if kind != ContactChangeKind::Create {
            let linked = at
                .and_then(|at| mailbox.contact_changes.get(at))
                .map(|c| c.remote.clone())
                .or_else(|| mailbox.contact(id).map(|c| c.origin.id.clone()));
            let named = !outcome.op.remote.is_empty() && (outcome.remote.is_empty() || outcome.remote == outcome.op.remote);
            if !named || linked.is_some_and(|linked| linked != outcome.op.remote) {
                continue;
            }
        }
        let hold = |change: &mut ContactChange| {
            change.attempts = change.attempts.saturating_add(1);
            change.next_try = now.saturating_add(wait_after(change.attempts, outcome.retry_after));
            change.error.clone_from(&outcome.error);
        };
        let denied_before = at.and_then(|at| mailbox.contact_changes.get(at)).is_some_and(|c| c.denied);
        // A folder of somebody else's (a shared mailbox's, a person's): may it be changed?
        let theirs = at
            .and_then(|at| mailbox.contact_changes.get(at))
            .and_then(|c| mailbox.contact_folder(c.folder))
            .filter(|f| !f.owner.is_own())
            .map(|f| f.id);
        // (Made, but under what name wasn't said: tried again, as for a failure.)
        let unnamed = outcome.result == ContactResult::Done && kind == ContactChangeKind::Create && outcome.remote.is_empty();
        match outcome.result {
            ContactResult::Failed => {
                if let Some(change) = at.and_then(|at| mailbox.contact_changes.get_mut(at)) {
                    hold(change);
                    // It may have reached the server, unless the server said it took nothing
                    // (too many requests: the rest of the batch wasn't tried either).
                    if kind == ContactChangeKind::Create && outcome.status != 429 {
                        (change.sent, change.tried, change.base) = (true, now, Some(Box::new(contact_from_remote(&outcome.op.values))));
                    }
                    applied.failed += 1;
                }
            }
            ContactResult::Done if unnamed => {
                // Made, but under a name not heard: not sent again (it would be made twice) until
                // a download shows it ([`merge_contacts`] takes it for this one) or doesn't.
                if let Some(change) = at.and_then(|at| mailbox.contact_changes.get_mut(at)) {
                    (change.unconfirmed, change.sent, change.tried) = (now.max(1), true, now);
                    change.base = Some(Box::new(contact_from_remote(&outcome.op.values)));
                    applied.done += 1;
                }
            }
            // Somebody else's folder that refuses a change: it can only be read, from now on (C5).
            ContactResult::Denied if theirs.is_some() => {
                give_up(mailbox, at, &outcome, &mut applied);
                refused.extend(theirs);
            }
            ContactResult::Denied if !denied_before => {
                if let Some(change) = at.and_then(|at| mailbox.contact_changes.get_mut(at)) {
                    change.denied = true;
                    hold(change);
                    applied.denied = true;
                    applied.failed += 1;
                }
            }
            ContactResult::Done => {
                let server = match kind {
                    // Taken already for one the server listed (a download came first): as it is.
                    ContactChangeKind::Create if at.is_none() && mailbox.contact(id).is_some_and(|c| !c.origin.id.is_empty()) => {
                        server_copy(mailbox, id)
                    }
                    ContactChangeKind::Create => {
                        let mut made = contact_from_remote(&outcome.op.values);
                        made.origin = Origin { account, id: outcome.remote.clone(), change_key: outcome.change_key.clone(), ..Default::default() };
                        let noted = at.and_then(|at| mailbox.contact_changes.get(at)).map(|c| c.folder);
                        // Undone meanwhile, the note gone with it: the folder it was sent to, so
                        // that the server is told to delete it again.
                        let sent_to = || mailbox.contact_folder_named(account, &outcome.op.owner, &outcome.op.folder);
                        made.folder = noted.or_else(|| mailbox.contact(id).map(|c| c.folder).filter(|f| *f != 0)).or_else(sent_to).unwrap_or(0);
                        Some(made)
                    }
                    ContactChangeKind::Change => {
                        let Some(mut base) = server_copy(mailbox, id) else { continue };
                        // The server's copy is what was sent, under the key it answered with,
                        // unless a download since brought a newer copy: then that copy, fields
                        // and key, is what it has.
                        let sent = contact_from_remote(&outcome.op.values);
                        if base.origin.change_key == outcome.op.values.change_key {
                            for field in &outcome.op.fields {
                                field.copy(&sent, &mut base);
                            }
                            if !outcome.change_key.is_empty() {
                                base.origin.change_key.clone_from(&outcome.change_key);
                            }
                        } else if let Some(here) = mailbox.contact(id) {
                            // A download came first (its key stands). What the server took and is
                            // still so here is through: it isn't sent again. A field changed again
                            // here, or one the server has otherwise, is left as the download says.
                            for field in outcome.op.fields.iter().filter(|f| !f.differs(here, &sent)) {
                                field.copy(&sent, &mut base);
                            }
                        }
                        Some(base)
                    }
                    ContactChangeKind::Delete => None,
                };
                if note_contact(mailbox, id, server).is_err() {
                    applied.say("Too many contact changes are waiting to be sent to the server. Let Send/Receive get them through first.".into());
                }
                applied.done += 1;
            }
            ContactResult::Gone if kind == ContactChangeKind::Delete => {
                let _ = note_contact(mailbox, id, None);
                applied.done += 1;
            }
            ContactResult::Gone if kind == ContactChangeKind::Change && at.is_some() => {
                // Deleted there, changed here: made again.
                if let Ok(c) = mailbox.contact_mut(id) {
                    c.origin.id.clear();
                    c.origin.change_key.clear();
                }
                let _ = note_contact(mailbox, id, None);
                applied.done += 1;
            }
            _ => give_up(mailbox, at, &outcome, &mut applied),
        }
    }
    for f in mailbox.contact_folders.iter_mut().filter(|f| refused.contains(&f.id)) {
        f.writable = false;
    }
    applied.waiting = s.mailbox.contact_changes.iter().filter(|c| c.account == account).count();
    if !uploaded.outcomes.is_empty() {
        s.mailbox_changed();
    }
    Ok(applied)
}

/// The server refused the change at `at` for good: the contact is as the server has it again
/// (one it was to make is kept on this computer), and the owner is told.
fn give_up(mailbox: &mut Mailbox, at: Option<usize>, outcome: &mailcraft_model::contact_sync::ContactOutcome, applied: &mut ContactsApplied) {
    let Some(at) = at.filter(|at| *at < mailbox.contact_changes.len()) else { return };
    let change = mailbox.contact_changes.remove(at);
    applied.refused += 1;
    let why = if outcome.error.trim().is_empty() { String::new() } else { format!(" ({})", outcome.error.trim()) };
    let folder = mailbox.contact_folder(change.folder).map(|f| f.name.clone()).unwrap_or_default();
    match change.kind {
        ContactChangeKind::Create => {
            let name = called(mailbox.contact(change.contact));
            // Moved from a server's folder: back there rather than in neither.
            if let Some(from) = super::contacts_move::moved_back(mailbox, change.contact) {
                applied.say(said!("{name} couldn't be moved to {folder}{why}: it stays in {from}.", name, folder, why, from).to_string());
                return;
            }
            if let Ok(c) = mailbox.contact_mut(change.contact) {
                (c.folder, c.origin, c.moved_from) = (0, Origin::default(), 0);
            }
            applied.say(said!("{name} couldn't be added to {folder}{why}: it is kept on this computer.", name, folder, why).to_string());
        }
        ContactChangeKind::Change => {
            let Some(base) = change.base else { return };
            let name = called(mailbox.contact(change.contact));
            // Only what was sent and refused goes back, and only where the owner hasn't changed
            // it again since; anything else changed here still waits to go up.
            let sent = contact_from_remote(&outcome.op.values);
            if let Some(now) = mailbox.contact(change.contact).cloned()
                && let Ok(c) = mailbox.contact_mut(change.contact)
            {
                let mut back = now.clone();
                for field in &outcome.op.fields {
                    if !field.differs(&now, &sent) {
                        field.copy(&base, &mut back);
                    }
                }
                *c = back;
                // (The refused note is gone: there is room for this one.)
                let _ = note_contact(mailbox, change.contact, Some(*base));
            }
            applied.say(said!("The server didn't take the change to {name}{why}: it is as the server has it again.", name, why).to_string());
        }
        ContactChangeKind::Delete => {
            let Some(base) = change.base else { return };
            let name = called(Some(&base));
            mailbox.add_contact(Contact { folder: change.folder, ..*base });
            applied.say(said!("The server didn't delete {name}{why}: it is back.", name, why).to_string());
        }
    }
}

/// Most lines said for contacts kept on this computer as their folder goes.
const MAX_KEPT_LINES: usize = 10;

/// Drop the contacts folders of `account` whose address books a server listed in full no longer
/// has (`listed`: each owner and the books it has, the default one as empty). A main folder stays.
/// Their contacts go with them, except one with a change made here not yet sent, which is kept on
/// this computer with a line saying so (never lost). Whether anything changed, and the lines.
pub(crate) fn drop_gone_folders(mailbox: &mut Mailbox, account: Id, listed: &[JmapContactPlace]) -> (bool, Vec<String>) {
    let gone: HashMap<Id, String> = mailbox
        .contact_folders
        .iter()
        .filter(|f| f.account == account && !f.is_main() && listed.iter().any(|p| p.owner == f.owner && !p.books.contains(&f.remote)))
        .map(|f| (f.id, f.name.clone()))
        .collect();
    if gone.is_empty() {
        return (false, Vec::new());
    }
    let kept: HashSet<Id> =
        mailbox.contact_changes.iter().filter(|c| gone.contains_key(&c.folder) && c.kind != ContactChangeKind::Delete).map(|c| c.contact).collect();
    mailbox.contact_changes.retain(|c| !gone.contains_key(&c.folder));
    mailbox.contact_folders.retain(|f| !gone.contains_key(&f.id));
    let contacts: Vec<(Id, Id)> = mailbox.contacts.values().filter(|c| gone.contains_key(&c.folder)).map(|c| (c.id, c.folder)).collect();
    let mut lines = Vec::new();
    for (id, folder) in contacts {
        if !kept.contains(&id) {
            let _ = mailbox.remove_contact(id);
            continue;
        }
        let name = called(mailbox.contact(id));
        if let Ok(c) = mailbox.contact_mut(id) {
            (c.folder, c.origin) = (0, Origin::default());
        }
        // lean: a line each, the first few; the rest are kept as quietly.
        if lines.len() < MAX_KEPT_LINES {
            let folder = gone.get(&folder).cloned().unwrap_or_default();
            lines.push(
                said!("{name} is kept on this computer: {folder} is no longer on the server, and the change made here couldn't go up.", name, folder)
                    .to_string(),
            );
        }
    }
    (true, lines)
}

fn contact_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::req_id(v, "account")?;
    serde_json::to_value(contact_upload(s, account)?).map_err(|e| fail(e.to_string()))
}

fn apply_contact_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let result = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let uploaded: ContactsUploaded = serde_json::from_value(result)
        .map_err(|e| bad(format!("`result`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))?;
    Ok(apply_contact_outcomes(s, &uploaded)?.json())
}

#[cfg(test)]
#[path = "contacts_remote_tests.rs"]
mod contacts_remote_tests;
