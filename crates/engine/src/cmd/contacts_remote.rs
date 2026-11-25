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
    ContactChange, ContactChangeKind, ContactField, ContactFolder, ContactOp, ContactResult, ContactsUpload, ContactsUploaded, MAX_CONTACT_CHANGES,
    MAX_CONTACT_OPS, RemoteContactFolder, delta, wait_after,
};
use mailcraft_model::remote::{ContactsDownload, MAX_CONTACTS, RemoteContact, clean_key};
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
            .params(r#"{"account": number} — what that account's server is to be told now of contacts made, changed or deleted here, oldest first: {"account", "email", "ops": [{"kind": "create"|"change"|"delete", "contact": number, "remote"?: string, "folder"?: string (the server's folder; none: the main one), "owner"?: {"mailbox": address} | {"person": address} (none: the account's own), "fields"?: ["displayName"|"first"|"middle"|"last"|"nickname"|"company"|"title"|"department"|"emails"|"phones"|"addresses"|"websites"|"birthday"|"notes"|"categories"] (only these are written), "values": RemoteContact}]}"#),
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
    // Where it is here: on a server that may change it, on one that may not (nothing to tell),
    // or on this computer (or nowhere).
    let place = here.as_ref().and_then(|c| home(mailbox, c.folder)).map(|f| (f.id, f.account, f.writable));
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
                // refusal of the permission once is remembered.
                if old.kind == change.kind {
                    (change.attempts, change.denied) = (old.attempts, old.denied);
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
/// contact's note, and the folders, are as `now` has them. How many couldn't be noted for want
/// of room.
pub(crate) fn after_undo(mailbox: &mut Mailbox, now: &Mailbox, touched: &BTreeSet<Id>) -> usize {
    mailbox.contact_folders.clone_from(&now.contact_folders);
    mailbox.contact_changes = now.contact_changes.iter().filter(|c| !touched.contains(&c.contact)).cloned().collect();
    let mut no_room = 0;
    for id in touched {
        if note_contact(mailbox, *id, server_copy(now, *id)).is_err() {
            no_room += 1;
        }
    }
    no_room
}

/// The folder a new contact goes into when the command doesn't say (C8), without making
/// anything: with one account that keeps contacts, its main folder; with more, the account the
/// list shows (On this computer when it shows that), else the folder last used, else the first
/// account's. `Err(account)` is that account's main folder, not made yet.
pub fn default_contact_folder(s: &Session) -> Result<Id, Id> {
    let accounts = super::people::contact_accounts(&s.mailbox);
    let usable = |f: Id| f == 0 || home(&s.mailbox, f).is_some_and(|f| f.writable);
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
    if categories {
        fresh.categories.clone_from(&old.categories);
    }
}

/// The folder a download is of, made or brought up to date here. Nothing is changed when this
/// fails.
fn take_folder(mailbox: &mut Mailbox, account: Id, listed: Option<&RemoteContactFolder>) -> Result<Id, CmdError> {
    let too_many = || fail("this account has too many contacts folders");
    let Some(listed) = listed else { return mailbox.ensure_main_contact_folder(account).ok_or_else(too_many) };
    let folder = ContactFolder {
        account,
        remote: listed.id.clone(),
        name: listed.name.clone(),
        owner: listed.owner.clone(),
        writable: listed.writable,
        ..Default::default()
    }
    .tidied()
    .ok_or_else(|| bad("the contacts folder's owner isn't an e-mail address"))?;
    let parent = clean_key(&listed.parent);
    let parent = if parent.is_empty() { 0 } else { mailbox.contact_folder_named(account, &folder.owner, &parent).unwrap_or(0) };
    match mailbox.contact_folder_named(account, &folder.owner, &folder.remote) {
        Some(id) => {
            if let Some(f) = mailbox.contact_folders.iter_mut().find(|f| f.id == id) {
                (f.name, f.writable, f.parent) = (folder.name, folder.writable, parent);
            }
            Ok(id)
        }
        None => mailbox.add_contact_folder(ContactFolder { parent, ..folder }).ok_or_else(too_many),
    }
}

/// Two contacts are the same person for taking in one made here whose answer was lost.
fn same_person(a: &Contact, b: &Contact) -> bool {
    a.name() == b.name() && a.primary_email() == b.primary_email()
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
    let account = owner(mailbox, download.account, &download.email)?.id;
    let folder = take_folder(mailbox, account, download.folder.as_ref())?;
    let mut merged = Merged::default();
    let queue = mailbox.contact_changes.clone();
    let mut known: HashMap<String, Id> = HashMap::new();
    let mut extra: Vec<Id> = Vec::new();
    for c in mailbox.contacts.values().filter(|c| c.origin.account == account && !c.origin.id.is_empty() && home(mailbox, c.folder).is_some()) {
        extra.extend(known.insert(c.origin.id.clone(), c.id));
    }
    // Deleted here and not told yet: still listed there, and not brought back.
    let deleting: HashSet<String> =
        mailbox.contact_changes.iter().filter(|c| c.account == account && c.kind == ContactChangeKind::Delete).map(|c| c.remote.clone()).collect();
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
                if remote.categories.is_none() {
                    fresh.categories = had;
                }
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
                if mine.is_some() {
                    // (The note is there already: there is room for it.)
                    let _ = note_contact(mailbox, old.id, Some(fresh));
                }
            }
            None => {
                let making = |c: &&ContactChange| c.account == account && c.kind == ContactChangeKind::Create && c.folder == folder;
                let twin = mailbox
                    .contact_changes
                    .iter()
                    .filter(making)
                    .filter_map(|c| mailbox.contact(c.contact))
                    .find(|c| same_person(c, &fresh))
                    .map(|c| c.id);
                match twin {
                    Some(twin) => {
                        let _ = note_contact(mailbox, twin, Some(fresh));
                        merged.updated += 1;
                    }
                    None => {
                        mailbox.add_contact(fresh);
                        merged.added += 1;
                    }
                }
            }
        }
    }
    // What the server no longer has.
    let mut gone: HashSet<String> = download.gone.iter().map(|id| clean_key(id)).filter(|id| !id.is_empty() && !seen.contains(id)).collect();
    if download.complete && download.contacts.len() <= MAX_CONTACTS {
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
        mailbox.contact_changes.retain(|c| !(c.account == account && c.kind == ContactChangeKind::Delete && c.remote == remote));
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
    if !position.is_empty()
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
/// [`MAX_CONTACT_OPS`] at most.
pub fn contact_upload(s: &Session, account: Id) -> Result<ContactsUpload, CmdError> {
    let owner = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    let (mailbox, now) = (&s.mailbox, s.now());
    let mut ops = Vec::new();
    for change in mailbox.contact_changes.iter().filter(|c| c.account == account && c.next_try <= now) {
        if ops.len() >= MAX_CONTACT_OPS {
            break;
        }
        let Some(folder) = mailbox.contact_folder(change.folder).filter(|f| f.account == account) else { continue };
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
/// again at once.
pub fn retry_contact_changes(s: &mut Session, account: Id) {
    let mut any = false;
    for change in s.mailbox.contact_changes.iter_mut().filter(|c| c.account == account && c.next_try != 0) {
        change.next_try = 0;
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
fn called(c: Option<&Contact>) -> String {
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
    for outcome in uploaded.outcomes.iter().take(MAX_OUTCOMES) {
        let mut outcome = outcome.clone();
        outcome.tidy();
        let (id, kind) = (outcome.op.contact, outcome.op.kind);
        let at = mailbox.contact_changes.iter().position(|c| c.account == account && c.contact == id && c.kind == kind);
        let hold = |change: &mut ContactChange| {
            change.attempts = change.attempts.saturating_add(1);
            change.next_try = now.saturating_add(wait_after(change.attempts, outcome.retry_after));
            change.error.clone_from(&outcome.error);
        };
        let denied_before = at.and_then(|at| mailbox.contact_changes.get(at)).is_some_and(|c| c.denied);
        // (Made, but under what name wasn't said: tried again, as for a failure.)
        let unnamed = outcome.result == ContactResult::Done && kind == ContactChangeKind::Create && outcome.remote.is_empty();
        match outcome.result {
            ContactResult::Failed => {
                if let Some(change) = at.and_then(|at| mailbox.contact_changes.get_mut(at)) {
                    hold(change);
                    applied.failed += 1;
                }
            }
            ContactResult::Done if unnamed => {
                if let Some(change) = at.and_then(|at| mailbox.contact_changes.get_mut(at)) {
                    hold(change);
                    applied.failed += 1;
                }
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
                    ContactChangeKind::Create => {
                        let mut made = contact_from_remote(&outcome.op.values);
                        made.origin = Origin { account, id: outcome.remote.clone(), change_key: outcome.change_key.clone(), ..Default::default() };
                        let noted = at.and_then(|at| mailbox.contact_changes.get(at)).map(|c| c.folder);
                        made.folder = noted.or_else(|| mailbox.contact(id).map(|c| c.folder)).unwrap_or(0);
                        Some(made)
                    }
                    ContactChangeKind::Change => {
                        let Some(mut base) = server_copy(mailbox, id) else { continue };
                        let sent = contact_from_remote(&outcome.op.values);
                        for field in &outcome.op.fields {
                            field.copy(&sent, &mut base);
                        }
                        if !outcome.change_key.is_empty() {
                            base.origin.change_key.clone_from(&outcome.change_key);
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
            if let Ok(c) = mailbox.contact_mut(change.contact) {
                (c.folder, c.origin) = (0, Origin::default());
            }
            applied.say(said!("{name} couldn't be added to {folder}{why}: it is kept on this computer.", name, folder, why).to_string());
        }
        ContactChangeKind::Change => {
            let Some(base) = change.base else { return };
            let name = called(mailbox.contact(change.contact));
            if let Some(now) = mailbox.contact(change.contact).cloned()
                && let Ok(c) = mailbox.contact_mut(change.contact)
            {
                let mut back = *base;
                keep_local(&mut back, &now, false);
                back.folder = now.folder;
                *c = back;
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
