// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contacts of an account that writes them to its server (an account kept in step over JMAP,
//! [`mailcraft_model::Account::writes_contacts`]), kept in step both ways.
//!
//! The engine does no I/O: a host with a way to the server asks what is to be told
//! (`sync.contactChanges`, [`contact_upload`]), tells it, says what the server made of it
//! (`sync.applyContactChanges`, [`apply_contact_outcomes`]) and then brings what the server has
//! (`sync.applyContacts`, [`super::remote::merge_contacts`]). The outcomes are taken in before the
//! download, which was read after the changes were told.
//!
//! **What a contact remembers.** A contact the server keeps has its [`Origin`] (the account and
//! the server's id) and `synced`: the contact as the server last had it. What was changed here
//! since is the difference between the two, part by part ([`ContactField`]), and only those parts
//! are sent. A contact made in the account has the account and no id until the server has given
//! it one. A contact deleted here is remembered ([`ContactGone`]) until the server has been told.
//!
//! **Conflicts.** A download is merged part by part against `synced`: a part that was changed here
//! stays as it is here (and goes up), every other part is the server's. A contact the server no
//! longer has goes from here too, unless it was changed here since: then it is made again. A
//! contact deleted here is not brought back by a download that still shows it. A contact the
//! server is yet to be given is never removed by a download, and one the download shows that is
//! the same person is that contact (the answer to making it was lost), not a second one.
//!
//! **What a server refuses** is not sent again as it is: the refused parts of a change are put
//! back to what the server has, a new contact it won't take stays on this computer, and a contact
//! it won't delete comes back with the next download. The owner is told each time.
//!
//! Everything from outside is hostile: ids and text are cut by [`RemoteContact::tidy`] and
//! [`mailcraft_model::remote::ContactOutcome::tidy`], counts are capped, and an answer for another account is refused whole.

use std::collections::{HashMap, HashSet};

use mailcraft_model::remote::{
    ContactField, ContactOp, ContactOpKind, ContactResult, ContactsDownload, ContactsUpload, ContactsUploaded, MAX_CONTACT_OPS, MAX_CONTACTS,
    RemoteContact, clean_key,
};
use mailcraft_model::{Account, Contact, ContactGone, Id, Labeled, MAX_CONTACT_GONE, Mailbox, Origin, PostalAddress};
use serde_json::{Value, json};

use super::remote::{Merged, contact_from_remote, owner, who};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session};

/// Most outcomes taken from one answer, and the longest text of a parameter error handed back.
const MAX_OUTCOMES: usize = 1_000;
const MAX_SHOWN: usize = 200;
/// Most new contacts one upload makes (the rest go with the next), as for to-dos.
pub const MAX_CONTACT_CREATES: usize = 50;
/// The longest a contact's name is in what the owner is told.
const MAX_NAME_SHOWN: usize = 80;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.contactChanges", "Contact Changes to Send", "Send / Receive", contact_changes_command)
            .pure()
            .params(r#"{"account": number}: what that account's server is to be told of contacts made, changed or deleted here (an account kept in step over JMAP; any other is refused): {"account", "email", "ops": [{"kind": "create"|"update"|"delete", "contact": number (0 for a deletion), "id"?: string (the server's; none for a creation), "card": RemoteContact (the contact as it is here; empty for a deletion), "changed"?: ["name"|"nickname"|"organization"|"title"|"emails"|"phones"|"addresses"|"websites"|"birthday"|"notes"] (for an update: the parts to write, and no others)}]}. At most 100 changes, the rest with the next"#),
        CommandSpec::new("sync.applyContactChanges", "Apply Sent Contact Changes", "Send / Receive", apply_contact_changes_command)
            .pure()
            .params(r#"{"result": ContactsUploaded, "sent"?: ContactsUpload}: what a host heard from the server: {"account": number, "email"?: string, "outcomes": [{"kind": "create"|"update"|"delete", "contact": number, "id"?: string (for a creation that is done: the id the server gave), "result": "done"|"gone"|"refused"|"failed", "changed"?: [the parts written], "error"?: string}]}. To be taken in before the contacts read after it (sync.applyContacts). "sent": the answer of sync.contactChanges that was carried out, when the host still has it: with it a change made here while the answer was on its way is told apart from what was sent; without it a change that is done stays noted until the next download shows it. Answers {"done", "failed", "messages": [string]}"#),
    ]
}

/// Whether what is changed of the account's contacts here is told to its server.
pub fn writes_contacts(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.account(account).is_some_and(Account::writes_contacts)
}

/// Whether `id` is something a server may call a contact: not empty, and unchanged by the cut
/// every id from outside gets.
fn named(id: &str) -> bool {
    !id.is_empty() && clean_key(id) == id
}

/// The server's ids of the contacts of `account` that were deleted here and wait to be deleted
/// there, each once. One that is here again (a mailbox file may say anything) is not deleted.
fn deletions(mailbox: &Mailbox, account: Id) -> Vec<&str> {
    let here: HashSet<&str> = mailbox.contacts.values().filter(|c| c.origin.account == account).map(|c| c.origin.id.as_str()).collect();
    let mut told: HashSet<&str> = HashSet::new();
    let waiting = mailbox.contact_gone.iter().take(MAX_CONTACT_GONE).filter(|gone| gone.account == account);
    waiting.map(|gone| gone.id.as_str()).filter(|id| named(id) && !here.contains(id) && told.insert(id)).collect()
}

/// The contacts of `account` the server has that were changed here since.
fn changed_here(mailbox: &Mailbox, account: Id) -> impl Iterator<Item = (&Contact, Vec<ContactField>)> {
    let theirs = mailbox.contacts.values().map(|c| c.as_ref()).filter(move |c| c.origin.account == account && named(&c.origin.id));
    theirs.map(|c| (c, c.changed_fields())).filter(|(_, changed)| !changed.is_empty())
}

/// The contacts of `account` its server is yet to be given, each as it would be sent. One that
/// would be nobody there (no name, company or address is left of it once it is cut to size) is
/// not sent: the next download could not show it.
fn made_here(mailbox: &Mailbox, account: Id) -> impl Iterator<Item = (&Contact, RemoteContact)> {
    let waiting = mailbox.contacts.values().map(|c| c.as_ref()).filter(move |c| c.origin.account == account && c.origin.id.is_empty());
    waiting.map(|c| (c, c.as_remote())).filter(|(_, card)| !nobody(card))
}

/// Whether nothing of `card` would name it in a list: no name, no company, no address
/// ([`Contact::name`] of the contact it is would be empty).
fn nobody(card: &RemoteContact) -> bool {
    [&card.display_name, &card.first, &card.middle, &card.last, &card.company].into_iter().all(|text| text.trim().is_empty())
        && card.emails.first().is_none_or(|entry| entry.value.is_empty())
}

/// How many changes wait to be told to `account`'s server; none for an account that doesn't
/// write its contacts.
pub fn contacts_waiting(mailbox: &Mailbox, account: Id) -> usize {
    if !writes_contacts(mailbox, account) {
        return 0;
    }
    deletions(mailbox, account).len() + changed_here(mailbox, account).count() + made_here(mailbox, account).count()
}

/// What `account`'s server is to be told: the deletions that wait, then what was changed of the
/// contacts it has, then the contacts made here, at most [`MAX_CONTACT_OPS`] in all.
pub fn contact_upload(mailbox: &Mailbox, account: Id) -> Result<ContactsUpload, CmdError> {
    let Some(a) = mailbox.account(account).filter(|a| a.writes_contacts()) else {
        return Err(bad("that account's contacts aren't written to its server"));
    };
    let mut ops: Vec<ContactOp> = Vec::new();
    for id in deletions(mailbox, account).into_iter().take(MAX_CONTACT_OPS) {
        ops.push(ContactOp { kind: ContactOpKind::Delete, id: id.to_string(), ..Default::default() });
    }
    for (contact, changed) in changed_here(mailbox, account) {
        if ops.len() >= MAX_CONTACT_OPS {
            break;
        }
        ops.push(ContactOp { kind: ContactOpKind::Update, contact: contact.id, id: contact.origin.id.clone(), card: contact.as_remote(), changed });
    }
    for (contact, card) in made_here(mailbox, account).take(MAX_CONTACT_CREATES) {
        if ops.len() >= MAX_CONTACT_OPS {
            break;
        }
        ops.push(ContactOp { kind: ContactOpKind::Create, contact: contact.id, card, ..Default::default() });
    }
    Ok(ContactsUpload { account, email: a.email.trim().to_string(), ops })
}

/// Whether `upload` tells the server to delete a contact: the window forgets the undo history as
/// the job that carries it starts ([`mark_sent`]).
pub fn carries_delete(upload: &ContactsUpload) -> bool {
    upload.ops.iter().any(|op| op.kind == ContactOpKind::Delete)
}

/// Forget the undo history as the job that sends an upload starts, when it carries a deletion
/// ([`carries_delete`]): the server is told to drop that contact, and going back to a mailbox from
/// before would bring it back here alone, to be taken away again by the next read. As for to-dos
/// ([`super::tasks_remote::mark_sent`]). Not an undo step.
pub fn mark_sent(s: &mut Session, deletes: bool) {
    if deletes {
        s.forget_history();
    }
}

/// A contact of an account that writes its contacts is about to be changed here (`before` is
/// what it was, `after` what it becomes): when the server has it and nothing says yet what the
/// server has (a contact read before changes were written, or by another version), what it was
/// is taken for that, so that the change is seen as one and goes up.
pub(crate) fn remember(mailbox: &Mailbox, before: &Contact, after: &mut Contact) {
    if after.synced.is_none() && !after.origin.id.is_empty() && writes_contacts(mailbox, after.origin.account) {
        after.synced = Some(Box::new(before.as_remote()));
    }
}

/// Refuse a deletion of `count` more contacts when too many already wait to be told to a server.
pub(crate) fn room_for_deletions(mailbox: &Mailbox, count: usize) -> Result<(), CmdError> {
    if mailbox.contact_gone.len().saturating_add(count) > MAX_CONTACT_GONE {
        return Err(failed!("Too many deleted contacts are waiting to be told to the server. Let Send/Receive get them through first."));
    }
    Ok(())
}

/// Whether a contact made here and one a download shows are the same person, for taking the
/// server's as the one made here (the answer to making it was lost). Every part; addresses and
/// numbers by their values, whatever the server made of their labels.
fn same_person(here: &RemoteContact, there: &RemoteContact) -> bool {
    use ContactField::{Birthday, Name, Nickname, Notes, Organization, Title, Websites};
    let values = |list: &[Labeled]| list.iter().map(|entry| entry.value.trim().to_lowercase()).collect::<Vec<_>>();
    let places = |list: &[PostalAddress]| list.iter().map(PostalAddress::one_line).collect::<Vec<_>>();
    [Name, Nickname, Organization, Title, Websites, Birthday, Notes].into_iter().all(|field| field.same(here, there))
        && values(&here.emails) == values(&there.emails)
        && values(&here.phones) == values(&there.phones)
        && places(&here.addresses) == places(&there.addresses)
}

/// [`super::remote::merge_contacts`] for an account that writes its contacts (see the module).
/// Nothing is changed when this returns an error. Doing it twice changes nothing the second time.
pub(super) fn merge_written(mailbox: &mut Mailbox, account: Id, download: &ContactsDownload) -> Result<Merged, CmdError> {
    let mut merged = Merged::default();
    // (A mailbox file may say anything: what can't be a deletion of this account's is let go.)
    let noted = mailbox.contact_gone.len();
    mailbox.contact_gone.retain(|gone| gone.account != account || named(&gone.id));
    mailbox.contact_gone.truncate(MAX_CONTACT_GONE);
    merged.checked = mailbox.contact_gone.len() != noted;
    let mut known: HashMap<String, Id> = HashMap::new();
    let mut extra: Vec<Id> = Vec::new();
    for contact in mailbox.contacts.values().filter(|contact| contact.origin.account == account && !contact.origin.id.is_empty()) {
        extra.extend(known.insert(contact.origin.id.clone(), contact.id));
    }
    let told: HashSet<String> = mailbox.contact_gone.iter().filter(|gone| gone.account == account).map(|gone| gone.id.clone()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    for remote in download.contacts.iter().take(MAX_CONTACTS) {
        let mut remote = remote.clone();
        remote.tidy();
        // Somebody with no name, company or address would be an empty line in the list.
        if remote.id.is_empty() || nobody(&remote) || !seen.insert(remote.id.clone()) {
            merged.skipped += 1;
            continue;
        }
        // Deleted here, and the server is yet to hear of it: not brought back.
        if told.contains(&remote.id) {
            merged.skipped += 1;
            continue;
        }
        let origin = Origin { account, id: remote.id.clone(), change_key: remote.change_key.clone(), ..Default::default() };
        match known.get(&remote.id).and_then(|id| mailbox.contact(*id)) {
            Some(old) => {
                // A part changed here since the server last had the contact stays as it is here;
                // every other part is the server's.
                let here = old.as_remote();
                let mut fresh = old.clone();
                for field in ContactField::ALL {
                    if old.synced.as_deref().is_none_or(|base| field.same(base, &here)) {
                        fresh.set_part(field, &remote);
                    }
                }
                fresh.origin = origin;
                fresh.synced = Some(Box::new(remote));
                if fresh == *old {
                    merged.unchanged += 1;
                } else if let Ok(contact) = mailbox.contact_mut(fresh.id) {
                    *contact = fresh;
                    merged.updated += 1;
                }
            }
            None => {
                // A contact made here whose answer was lost (the server made it, the word didn't
                // come) is the same one: taken in, not made twice.
                let waiting = mailbox.contacts.values().filter(|c| c.origin.account == account && c.origin.id.is_empty());
                let twin = waiting.map(|c| (c.id, c.as_remote())).find(|(_, here)| same_person(here, &remote)).map(|(id, _)| id);
                if let Some(twin) = twin
                    && let Ok(contact) = mailbox.contact_mut(twin)
                {
                    // (What differs of it here, a label say, is a change made here.)
                    contact.origin = origin;
                    contact.synced = Some(Box::new(remote));
                    merged.updated += 1;
                    continue;
                }
                let mut fresh = contact_from_remote(&remote);
                fresh.origin = origin;
                fresh.synced = Some(Box::new(remote));
                mailbox.add_contact(fresh);
                merged.added += 1;
            }
        }
    }
    // What the server no longer has: only when the download says it showed everything.
    if download.complete && download.contacts.len() <= MAX_CONTACTS {
        let missing: Vec<Id> = known.iter().filter(|(remote, _)| !seen.contains(*remote)).map(|(_, id)| *id).collect();
        for id in missing {
            let changed = mailbox.contact(id).is_some_and(|contact| !contact.changed_fields().is_empty());
            if changed {
                // Changed here since: it is made again rather than lost.
                if let Ok(contact) = mailbox.contact_mut(id) {
                    contact.origin = Origin { account, ..Default::default() };
                    contact.synced = None;
                    merged.updated += 1;
                }
            } else if mailbox.remove_contact_quietly(id).is_ok() {
                merged.removed += 1;
            }
        }
        // (Two contacts under one id: a file from elsewhere. The one more goes.)
        for id in extra {
            if mailbox.remove_contact_quietly(id).is_ok() {
                merged.removed += 1;
            }
        }
    }
    Ok(merged)
}

/// Take in what a server made of the changes it was told ([`apply_contact_outcomes_of`]), for a
/// host that no longer has what it sent. Not an undo step.
pub fn apply_contact_outcomes(mailbox: &mut Mailbox, uploaded: &ContactsUploaded) -> Result<Merged, CmdError> {
    apply_contact_outcomes_of(mailbox, uploaded, None)
}

/// Take in what a server made of the changes it was told; `sent` is the upload that was carried
/// out, when the host still has it. Not an undo step.
///
/// - A new contact that is **done** takes the id the server gave it, and what was sent is what the
///   server has. One that was deleted here meanwhile is noted for the server to delete.
/// - A change that is **done**: the parts that were written are what the server has now, as they
///   were sent. A part changed again since stays to be told.
/// - A deletion that is **done**, or of a contact the server no longer has, is forgotten.
/// - A change to a contact the server **no longer has**: the contact is made again.
/// - **Refused**: see the module; never sent again as it is.
/// - **Failed**: nothing changes, and it goes again.
///
/// An outcome doesn't say what was sent. Without `sent`, a change that is done leaves what was
/// noted of the server as it is (the download read after it shows the server's side, and settles
/// it), and a new contact is noted as one the server has nothing of yet but its id: what is here
/// goes up once more unless a download shows it first. Nothing is lost either way.
pub fn apply_contact_outcomes_of(mailbox: &mut Mailbox, uploaded: &ContactsUploaded, sent: Option<&ContactsUpload>) -> Result<Merged, CmdError> {
    let owner = owner(mailbox, uploaded.account, &uploaded.email)?;
    if !owner.writes_contacts() {
        return Err(bad("that account's contacts aren't written to its server"));
    }
    let (account, server) = (owner.id, who(owner).to_string());
    let asked = sent.is_some();
    let sent: &[ContactOp] = sent.filter(|upload| upload.account == account).map_or(&[], |upload| upload.ops.as_slice());
    let mut merged = Merged::default();
    let mut failed = 0usize;
    for outcome in uploaded.outcomes.iter().take(MAX_OUTCOMES) {
        let mut outcome = outcome.clone();
        outcome.tidy();
        let op = sent
            .iter()
            .find(|op| op.kind == outcome.kind && op.contact == outcome.contact && (op.kind == ContactOpKind::Create || op.id == outcome.id));
        let why = if outcome.error.is_empty() { String::new() } else { format!(": {}", outcome.error) };
        let name =
            |mailbox: &Mailbox| mailbox.contact(outcome.contact).map(|c| mailcraft_names::shown(&c.name(), MAX_NAME_SHOWN)).unwrap_or_default();
        // The contact an outcome is about, when it is still this account's.
        let theirs = |c: &&Contact| c.origin.account == account;
        match (outcome.kind, outcome.result) {
            (ContactOpKind::Delete, ContactResult::Done | ContactResult::Gone | ContactResult::Refused) => {
                let noted = mailbox.contact_gone.len();
                mailbox.contact_gone.retain(|gone| !(gone.account == account && gone.id == outcome.id));
                if mailbox.contact_gone.len() == noted {
                    failed += 1;
                } else if outcome.result == ContactResult::Refused {
                    merged.say(format!(
                        "The server of {server} didn't delete a contact that was deleted here; it comes back with the next Send/Receive{why}"
                    ));
                    merged.put_back += 1;
                } else {
                    merged.removed += 1;
                }
            }
            (ContactOpKind::Create, ContactResult::Done) => {
                let taken = mailbox.contacts.values().any(|c| c.origin.account == account && c.origin.id == outcome.id);
                let waits = mailbox.contact(outcome.contact).map(|c| theirs(&c) && c.origin.id.is_empty());
                match waits {
                    _ if outcome.id.is_empty() || taken => failed += 1,
                    Some(true) => {
                        if let Ok(contact) = mailbox.contact_mut(outcome.contact) {
                            contact.origin = Origin { account, id: outcome.id.clone(), ..Default::default() };
                            // What was sent is what the server has; when nobody says what that
                            // was, the server is taken to have nothing of it yet.
                            let mut has = op.map(|op| op.card.clone()).unwrap_or_default();
                            has.tidy();
                            has.id = outcome.id.clone();
                            contact.synced = Some(Box::new(has));
                        }
                        merged.added += 1;
                    }
                    // (It has an id already: a download took it in first. Or it is not this
                    // account's to give one.)
                    Some(false) => failed += 1,
                    // (Not a creation that was asked for, where what was asked for is known.)
                    None if asked && op.is_none() => failed += 1,
                    None => {
                        // Deleted here while it was being made: the server is told to delete it.
                        let noted = mailbox.contact_gone.iter().any(|gone| gone.account == account && gone.id == outcome.id);
                        if !noted && mailbox.contact_gone.len() < MAX_CONTACT_GONE {
                            mailbox.contact_gone.push(ContactGone { account, id: outcome.id.clone() });
                            merged.checked = true;
                        }
                    }
                }
            }
            (ContactOpKind::Update, ContactResult::Done) => {
                let is_it = |c: &&Contact| theirs(c) && c.origin.id == outcome.id;
                if mailbox.contact(outcome.contact).filter(is_it).is_none() {
                    failed += 1;
                } else if let (Some(op), Ok(contact)) = (op, mailbox.contact_mut(outcome.contact)) {
                    // The parts that were written, as the op named them: all it named when the
                    // answer names none.
                    let written = |field: &&ContactField| outcome.changed.is_empty() || outcome.changed.contains(field);
                    let mut has = contact.synced.take().map_or_else(|| op.card.clone(), |has| *has);
                    for field in op.changed.iter().filter(written) {
                        field.copy(&op.card, &mut has);
                    }
                    has.tidy();
                    contact.synced = Some(Box::new(has));
                    merged.updated += 1;
                } else {
                    // (Nobody says what was sent: the download read after it settles it.)
                    merged.updated += 1;
                }
            }
            // The server no longer has it: made again, with what was changed here.
            (ContactOpKind::Update, ContactResult::Gone) => {
                let is_it = |c: &&Contact| theirs(c) && c.origin.id == outcome.id && !outcome.id.is_empty();
                if mailbox.contact(outcome.contact).filter(is_it).is_none() {
                    failed += 1;
                } else if let Ok(contact) = mailbox.contact_mut(outcome.contact) {
                    contact.origin = Origin { account, ..Default::default() };
                    contact.synced = None;
                    merged.updated += 1;
                }
            }
            // Refused: the server's copy stands. The refused parts are as the server has them
            // again (a part changed once more since it was sent stays, and is tried by itself).
            (ContactOpKind::Update, ContactResult::Refused) => {
                let is_it = |c: &&Contact| theirs(c) && c.origin.id == outcome.id;
                let name = name(mailbox);
                let Some(old) = mailbox.contact(outcome.contact).filter(is_it) else {
                    failed += 1;
                    continue;
                };
                let (here, pending) = (old.as_remote(), old.changed_fields());
                let refused: Vec<ContactField> = match op {
                    Some(op) => pending.iter().copied().filter(|field| op.changed.contains(field) && field.same(&op.card, &here)).collect(),
                    None if outcome.changed.is_empty() => pending,
                    None => pending.into_iter().filter(|field| outcome.changed.contains(field)).collect(),
                };
                if let (Some(has), Ok(contact)) = (old.synced.clone(), mailbox.contact_mut(outcome.contact)) {
                    for field in &refused {
                        contact.set_part(*field, &has);
                    }
                }
                merged.say(format!(
                    "The server of {server} didn't take a change to the contact \"{name}\"; what it refused is as the server has it again{why}"
                ));
                merged.put_back += 1;
            }
            (ContactOpKind::Create, ContactResult::Refused) => {
                let name = name(mailbox);
                let waits = |c: &&Contact| theirs(c) && c.origin.id.is_empty();
                if mailbox.contact(outcome.contact).filter(waits).is_none() {
                    failed += 1;
                } else if let Ok(contact) = mailbox.contact_mut(outcome.contact) {
                    contact.origin = Origin::default();
                    contact.synced = None;
                    merged.say(format!("The server of {server} didn't take the new contact \"{name}\"; it stays on this computer{why}"));
                    merged.put_back += 1;
                }
            }
            _ => failed += 1,
        }
    }
    merged.skipped = failed;
    Ok(merged)
}

/// Take in what a host heard from the server ([`apply_contact_outcomes_of`]). Not an undo step.
pub fn apply_contact_changes(s: &mut Session, uploaded: &ContactsUploaded, sent: Option<&ContactsUpload>) -> Result<Merged, CmdError> {
    let merged = apply_contact_outcomes_of(&mut s.mailbox, uploaded, sent)?;
    // What a server was told can't be undone there: going back to a mailbox from before would
    // undo it here only (to-dos and events do the same).
    if uploaded.outcomes.iter().any(|o| matches!(o.result, ContactResult::Done | ContactResult::Gone)) {
        s.forget_history();
    }
    if merged.changed() {
        s.mailbox_changed();
    }
    Ok(merged)
}

fn param<T: serde::de::DeserializeOwned>(v: &Value, name: &str) -> Result<T, CmdError> {
    let value = v.get(name).cloned().ok_or_else(|| bad(format!("`{name}` is required")))?;
    serde_json::from_value(value)
        .map_err(|e| bad(format!("`{name}`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))
}

fn contact_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = crate::p::req_id(v, "account")?;
    let upload = contact_upload(&s.mailbox, account)?;
    serde_json::to_value(upload).map_err(|e| fail(e.to_string()))
}

fn apply_contact_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let uploaded = param::<ContactsUploaded>(v, "result")?;
    let sent = match v.get("sent") {
        None | Some(Value::Null) => None,
        Some(_) => Some(param::<ContactsUpload>(v, "sent")?),
    };
    let merged = apply_contact_changes(s, &uploaded, sent.as_ref())?;
    Ok(json!({
        "done": merged.added + merged.updated + merged.removed, "failed": merged.skipped, "messages": merged.messages,
    }))
}
