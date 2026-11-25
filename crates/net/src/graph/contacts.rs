// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts kept in step with Microsoft Graph both ways (#1917).
//!
//! **Down** ([`read_contacts`]): every contacts folder of the account (its main one, the folders
//! under it, theirs, [`MAX_DEPTH`] deep), of each shared mailbox it opens, and the folders
//! shared with the owner that the mailbox knows already (Graph lists no such thing), each read
//! by its own list of changes (`/contactFolders/{id}/contacts/delta`). The first round, and one
//! whose saved position the server no longer has (410, or a `SyncState…` refusal), reads the
//! folder whole; later rounds bring only what changed, with the deleted ones by id
//! (`@removed`). A position is kept only from a round read to its end.
//!
//! **Up** ([`push_contacts`]): a contact made here is `POST`ed to its folder, one changed is
//! `PATCH`ed with the fields the owner changed and no other (so what Graph keeps and MailCraft
//! doesn't show is never written over; the e-mail addresses keep the names Graph has for them),
//! one deleted is `DELETE`d. What the server answered is mapped by
//! [`ContactResult::of_status`]; a 401 stops the upload, as does an answer that says to try later.
//!
//! What comes back is hostile JSON: folders, pages, bytes of a page, contacts of a folder are
//! capped, an entry that is no contact is passed over, and every id that goes into an address is
//! one piece of it. Nothing of a contact, and no id, is logged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use mailcraft_model::contact_sync::{
    ContactChangeKind, ContactField, ContactFolder, ContactOp, ContactOutcome, ContactResult, ContactsUpload, ContactsUploaded, FolderOwner,
    MAIN_FOLDER_NAME, MAX_CONTACT_FOLDERS, MAX_CONTACT_OPS, RemoteContactFolder,
};
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use serde_json::{Map, Value, json};

use super::{
    ASK_TIMEOUT, CONTACT_FIELDS, Doing, Graph, LIST_TIMEOUT, MAX_ENTRIES, MAX_PAGE, MAX_PAGES, MAX_WRITTEN, WRITE_TIMEOUT, code_name, contact_of,
    nonsense, page_from, person, shared_mailbox_path, text, wait_of,
};
use crate::NetError;
use crate::http::{self, Url};

/// Most contacts taken from one folder in one round; a folder that has more is read in part,
/// and nothing is removed for not being in it.
pub const MAX_FOLDER_CONTACTS: usize = 10_000;
/// How deep folders under a main contacts folder are looked for.
pub const MAX_DEPTH: usize = 4;
/// Longest id put into an address.
const MAX_ID: usize = 1_024;
/// How many of each Graph keeps of a contact: e-mail addresses, business and home numbers.
/// Entries asked for per page of a list of changes.
const DELTA_PAGE: &str = "odata.maxpagesize=50";

/// What [`read_contacts`] brought: a download per folder read, parents before their folders.
#[derive(Debug, Default)]
pub struct ContactsRead {
    pub downloads: Vec<ContactsDownload>,
    /// What couldn't be read, a line each, without a name or an id in it.
    pub errors: Vec<String>,
    /// Graph stopped taking the token (401): nothing more was asked.
    pub token_refused: bool,
}

/// Read the contacts folders of the owner of `token`, and of the shared mailboxes at
/// `mailboxes`, each by its list of changes from where `known` says it was read up to (see the
/// module). A folder `known` has as one the account may only read stays so (the server refused a
/// change to it once); one shared with the owner by a person is read when `known` has it.
pub fn read_contacts(token: &str, known: &[ContactFolder], mailboxes: &[String]) -> ContactsRead {
    read_contacts_at(&super::graph_base(), token, known, mailboxes)
}

/// Tell Graph what was done here to the contacts of `upload`, one op after the other; what it
/// made of each comes back. The rest of an upload that met a 401, or a server that said to try
/// later, is left untried (it waits, as it was).
pub fn push_contacts(token: &str, upload: &ContactsUpload) -> ContactsUploaded {
    push_contacts_at(&super::graph_base(), token, upload)
}

/// An id from Graph or the mailbox file that may go into an address: see `tasks::valid_id`.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !matches!(id, "." | "..") && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// `/v1.0/me`, or `/v1.0/users/{address}` for a mailbox that isn't the owner's.
fn mailbox_prefix(owner: &FolderOwner) -> Result<String, NetError> {
    match owner.address() {
        None => Ok("/v1.0/me".into()),
        Some(address) => Ok(format!("/v1.0{}", shared_mailbox_path(address)?)),
    }
}

pub(super) fn read_contacts_at(base: &str, token: &str, known: &[ContactFolder], mailboxes: &[String]) -> ContactsRead {
    let mut out = ContactsRead::default();
    let Ok(graph) = Graph::at(base, token) else {
        out.token_refused = true;
        return out;
    };
    // Whose folders: the owner's, each shared mailbox's, and each person's who shared one.
    let mut owners: Vec<FolderOwner> = vec![FolderOwner::Own];
    let shared = mailboxes.iter().map(|a| FolderOwner::Mailbox(a.clone()));
    let people = known.iter().filter(|f| matches!(f.owner, FolderOwner::Person(_))).map(|f| f.owner.clone());
    for owner in shared.chain(people) {
        if let Some(owner) = owner.tidied().filter(|o| !owners.contains(o)) {
            owners.push(owner);
        }
    }
    let mut room = MAX_CONTACT_FOLDERS;
    for owner in owners {
        if room == 0 || out.token_refused {
            break;
        }
        let Ok(prefix) = mailbox_prefix(&owner) else { continue };
        let saved = |remote: &str| known.iter().find(|f| f.owner == owner && f.remote == remote);
        let writable = |remote: &str| owner.is_own() || saved(remote).is_none_or(|f| f.writable);
        // The folders under the main one: listed (a person's: only those known here).
        let mut folders = vec![RemoteContactFolder {
            id: String::new(),
            name: MAIN_FOLDER_NAME.into(),
            owner: owner.clone(),
            writable: writable(""),
            ..Default::default()
        }];
        let listed = match &owner {
            FolderOwner::Person(_) => Ok(known
                .iter()
                .filter(|f| f.owner == owner && !f.remote.is_empty())
                .map(|f| RemoteContactFolder {
                    id: f.remote.clone(),
                    name: f.name.clone(),
                    owner: owner.clone(),
                    writable: f.writable,
                    parent: String::new(),
                })
                .collect()),
            _ => list_folders(&graph, &prefix, &owner, room.saturating_sub(1)),
        };
        match listed {
            Ok(list) => folders.extend(list.into_iter().map(|f| RemoteContactFolder { writable: writable(&f.id), ..f })),
            Err(e) if e.is_refused_sign_in() => {
                out.token_refused = true;
                break;
            }
            // A shared mailbox that doesn't let the owner in: its folders aren't read.
            Err(NetError::Forbidden(_)) if !owner.is_own() => {
                log::info!("contacts: a shared mailbox's contacts folders can't be listed with this sign-in");
                continue;
            }
            // (The main folder is read all the same: it doesn't need the list.)
            Err(e) => out.errors.push(format!("Couldn't list the contacts folders: {e}")),
        }
        for folder in folders.into_iter().take(room) {
            room = room.saturating_sub(1);
            let delta = saved(&folder.id).map_or("", |f| f.delta.as_str());
            // (When the reading started, before anything is asked: a new contact answered before
            // it and not in it wasn't made.)
            let read_at = mailcraft_time::now();
            match read_folder(&graph, &prefix, &folder.id, delta) {
                Ok(mut download) => {
                    download.read_at = read_at;
                    // (The account's own main folder is the one a download names by nothing.)
                    if !(owner.is_own() && folder.id.is_empty()) {
                        download.folder = Some(folder);
                    }
                    out.downloads.push(download);
                }
                Err(e) if e.is_refused_sign_in() => {
                    out.token_refused = true;
                    break;
                }
                Err(NetError::Forbidden(_)) if !owner.is_own() => log::info!("contacts: a shared contacts folder can't be read with this sign-in"),
                Err(e) => out.errors.push(format!("Couldn't bring the contacts down: {e}")),
            }
        }
    }
    log::debug!("contacts: {} folders read, {} problems", out.downloads.len(), out.errors.len());
    out
}

/// The folders under the main contacts folder at `prefix`, breadth first ([`MAX_DEPTH`] deep,
/// `most` at most), each naming the one above it (none for those right under the main one).
fn list_folders(graph: &Graph<'_>, prefix: &str, owner: &FolderOwner, most: usize) -> Result<Vec<RemoteContactFolder>, NetError> {
    let mut found: Vec<RemoteContactFolder> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, String, usize)> = VecDeque::from([(format!("{prefix}/contactFolders"), String::new(), 1)]);
    while let Some((path, parent, depth)) = queue.pop_front() {
        let room = most.saturating_sub(found.len());
        if room == 0 {
            break;
        }
        let first = graph.url(&format!("{path}?{}", http::form(&[("$top", "100"), ("$select", "id,displayName")])));
        let mut here: Vec<RemoteContactFolder> = Vec::new();
        graph.list(Doing::Contacts, first, &[], room, &mut |entry| {
            let id = text(entry, "id");
            if valid_id(id) && seen.insert(id.to_string()) {
                let name = text(entry, "displayName").to_string();
                here.push(RemoteContactFolder { id: id.to_string(), name, parent: parent.clone(), owner: owner.clone(), writable: true });
            }
        })?;
        for folder in &here {
            if depth < MAX_DEPTH {
                queue.push_back((format!("{prefix}/contactFolders/{}/childFolders", http::encode(&folder.id)), folder.id.clone(), depth + 1));
            }
        }
        found.extend(here);
    }
    Ok(found)
}

/// One folder's contacts: from the position `delta` when there is one that can be followed and
/// the server still has, else whole. `remote` is the folder's id; empty for the main one.
fn read_folder(graph: &Graph<'_>, prefix: &str, remote: &str, delta: &str) -> Result<ContactsDownload, NetError> {
    if let Some(url) = followable(graph, delta) {
        match delta_round(graph, url, false) {
            Err(Round::Gone) => log::info!("contacts: the server no longer has where a folder was read up to; it is read whole"),
            Err(Round::Failed(e)) => return Err(e),
            Ok(download) => return Ok(download),
        }
    }
    let id = if remote.is_empty() {
        match main_folder_id(graph, prefix)? {
            Some(id) => id,
            // A list that says, as a list, that it has no contacts (anything else is an error):
            // it is empty, and that is all of it. With no position: the next read is whole.
            None => return Ok(ContactsDownload { complete: true, ..Default::default() }),
        }
    } else {
        remote.to_string()
    };
    if !valid_id(&id) {
        return Err(nonsense("Microsoft Graph named a contacts folder that can't be asked for"));
    }
    let query = http::form(&[("$select", CONTACT_FIELDS)]);
    let first = graph.url(&format!("{prefix}/contactFolders/{}/contacts/delta?{query}", http::encode(&id)));
    delta_round(graph, first, true).map_err(|round| match round {
        Round::Gone => nonsense("Microsoft Graph has no list of changes for a contacts folder"),
        Round::Failed(e) => e,
    })
}

/// A saved position that may be followed: an address on Graph itself of a folder's list of
/// changes. Anything else (from a mailbox file that says anything) is read whole instead.
fn followable(graph: &Graph<'_>, delta: &str) -> Option<Url> {
    if delta.is_empty() {
        return None;
    }
    let url = Url::parse_within(delta, http::MAX_LONG_URL).ok()?;
    let path = url.path.split('?').next().unwrap_or("");
    (graph.base.same_server(&url) && path.starts_with("/v1.0/") && path.ends_with("/contacts/delta")).then_some(url)
}

/// What the main contacts folder at `prefix` is called by the server: the folder its first
/// contact is in. `None` when it has none.
fn main_folder_id(graph: &Graph<'_>, prefix: &str) -> Result<Option<String>, NetError> {
    let (mut id, mut listed) = (None, false);
    let first = graph.url(&format!("{prefix}/contacts?{}", http::form(&[("$top", "1"), ("$select", "parentFolderId")])));
    let ended = graph.list(Doing::Contacts, first, &[], 1, &mut |entry| {
        listed = true;
        let parent = text(entry, "parentFolderId");
        if valid_id(parent) {
            id = Some(parent.to_string());
        }
    })?;
    // A contact listed without a folder that can be asked for, or a list that didn't come to its
    // end: not an empty folder.
    if (listed || !ended) && id.is_none() {
        return Err(nonsense("Microsoft Graph didn't say which folder the contacts are in"));
    }
    Ok(id)
}

/// Why a round of a list of changes came to nothing.
enum Round {
    /// The server no longer has the position it was asked from.
    Gone,
    Failed(NetError),
}

/// Read a list of changes from `first` to its end: the contacts that changed (all of them, for a
/// `whole` round), those deleted, and the position to ask from next time.
fn delta_round(graph: &Graph<'_>, first: Url, whole: bool) -> Result<ContactsDownload, Round> {
    let started = Instant::now();
    let mut download = ContactsDownload::default();
    // (An entry can come more than once in a round: the last says how it is, a deletion too.)
    let mut kept: Vec<Option<RemoteContact>> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut gone: HashSet<String> = HashSet::new();
    let (mut unread, mut capped) = (0usize, false);
    let mut next = Some(first);
    for _ in 0..MAX_PAGES {
        let Some(url) = next.take() else { break };
        if started.elapsed() >= LIST_TIMEOUT {
            return Err(Round::Failed(nonsense("Microsoft Graph took too long to answer")));
        }
        let reply = graph.ask(&url, "GET", &[("Prefer", DELTA_PAGE)], None, ASK_TIMEOUT, MAX_PAGE).map_err(Round::Failed)?;
        if reply.status == 410
            || (reply.status == 400 && code_name(&reply.body, &[]).is_some_and(|code| code.to_ascii_lowercase().starts_with("syncstate")))
        {
            return Err(Round::Gone);
        }
        if reply.status != 200 {
            return Err(Round::Failed(graph.refusal(Doing::Contacts, &reply, "")));
        }
        let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        let link = |name: &str| json.get(name).and_then(Value::as_str).map(str::to_string);
        let (more, end) = (link("@odata.nextLink"), link("@odata.deltaLink"));
        let page = page_from(json).map_err(Round::Failed)?;
        for entry in &page.entries {
            let id = text(entry, "id");
            if entry.get("@removed").is_some() {
                if valid_id(id) {
                    if let Some(slot) = at.remove(id).and_then(|i| kept.get_mut(i)) {
                        *slot = None;
                    }
                    if gone.insert(id.to_string()) {
                        download.gone.push(id.to_string());
                    }
                }
                continue;
            }
            let Some(contact) = contact_of(entry) else {
                unread += 1;
                continue;
            };
            if gone.remove(&contact.id) {
                download.gone.retain(|g| *g != contact.id);
            }
            if let Some(slot) = at.get(&contact.id).and_then(|i| kept.get_mut(*i)) {
                *slot = Some(contact);
                continue;
            }
            if kept.len() >= MAX_FOLDER_CONTACTS {
                capped = true;
                break;
            }
            at.insert(contact.id.clone(), kept.len());
            kept.push(Some(contact));
        }
        if capped {
            break;
        }
        match (more, end) {
            (Some(more), _) => next = Some(graph.next_page(&Value::String(more), &url).map_err(Round::Failed)?),
            (None, Some(end)) => {
                // Where to ask from next time: only an address on Graph itself.
                download.delta = graph.next_page(&Value::String(end.clone()), &url).map(|_| end).unwrap_or_default();
            }
            (None, None) => {}
        }
    }
    download.contacts = kept.into_iter().flatten().collect();
    // (A round cut short keeps no position: the next one starts again.)
    // (Nor does a whole read that couldn't read every entry: the next starts whole again.)
    if capped || next.is_some() || download.delta.is_empty() || (whole && unread > 0) {
        download.delta.clear();
        download.complete = false;
    } else {
        download.complete = whole && unread == 0;
    }
    log::debug!(
        "contacts: a folder's changes came down: {} contacts, {} gone, {unread} unreadable, whole: {whole}, read to its end: {}",
        download.contacts.len(),
        download.gone.len(),
        !download.delta.is_empty()
    );
    Ok(download)
}

pub(super) fn push_contacts_at(base: &str, token: &str, upload: &ContactsUpload) -> ContactsUploaded {
    let mut uploaded = ContactsUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    let Ok(graph) = Graph::at(base, token) else {
        uploaded.token_refused = true;
        return uploaded;
    };
    for op in upload.ops.iter().take(MAX_CONTACT_OPS) {
        let Some(outcome) = push_one(&graph, token, op) else {
            uploaded.token_refused = true;
            break;
        };
        // Not now: the rest waits too.
        let stop = outcome.result == ContactResult::Failed;
        uploaded.outcomes.push(outcome);
        if stop {
            break;
        }
    }
    for outcome in &mut uploaded.outcomes {
        outcome.tidy();
    }
    uploaded
}

/// What the server made of one op; `None` when it didn't take the token (401).
fn push_one(graph: &Graph<'_>, token: &str, op: &ContactOp) -> Option<ContactOutcome> {
    let outcome = |result: ContactResult, status: u16, error: &str| ContactOutcome {
        op: op.clone(),
        result,
        status,
        error: error.into(),
        ..Default::default()
    };
    let refused = |error: &str| Some(outcome(ContactResult::Refused, 0, error));
    let Ok(prefix) = mailbox_prefix(&op.owner) else { return refused("that isn't the address of a mailbox") };
    if !op.folder.is_empty() && !valid_id(&op.folder) {
        return refused("that isn't a contacts folder the server could have");
    }
    if op.kind != ContactChangeKind::Create && !valid_id(&op.remote) {
        return refused("that isn't a contact the server could have");
    }
    let folder =
        if op.folder.is_empty() { format!("{prefix}/contacts") } else { format!("{prefix}/contactFolders/{}/contacts", http::encode(&op.folder)) };
    let item = format!("{folder}/{}", http::encode(&op.remote));
    let (method, path, body) = match op.kind {
        ContactChangeKind::Create => ("POST", folder, Some(contact_body(&op.values, &ContactField::ALL, &HashMap::new()))),
        ContactChangeKind::Change if op.fields.is_empty() => return refused("the change says nothing that could be sent"),
        ContactChangeKind::Change => {
            let fields: Vec<ContactField> = op.fields.iter().copied().collect();
            // The names Graph has for the addresses, kept for those that stay.
            let names = if op.fields.contains(&ContactField::Emails) {
                match email_names(graph, &item) {
                    Ok(names) => names,
                    Err((status, wait, why)) => {
                        let mut outcome = answered(op, (status, wait.as_deref()), &[], "", token)?;
                        if !why.is_empty() {
                            outcome.error = why;
                        }
                        return Some(outcome);
                    }
                }
            } else {
                HashMap::new()
            };
            ("PATCH", item, Some(contact_body(&op.values, &fields, &names)))
        }
        ContactChangeKind::Delete => ("DELETE", item, None),
    };
    let body = body.map(|b| b.to_string());
    let headers: &[(&str, &str)] = if body.is_some() { &[("Content-Type", "application/json")] } else { &[] };
    let reply = match graph.ask(&graph.url(&path), method, headers, body.as_deref(), WRITE_TIMEOUT, MAX_WRITTEN) {
        Ok(reply) => reply,
        Err(e) if e.is_refused_sign_in() => return None,
        Err(e) => return Some(outcome(ContactResult::Failed, 0, &e.to_string())),
    };
    let sent = body.as_deref().unwrap_or("");
    answered(op, (reply.status, reply.retry_after.as_deref()), &reply.body, sent, token)
}

/// The outcome of an answer with `status` (and what it said to wait) and `body`; `None` for 401.
fn answered(op: &ContactOp, (status, retry_after): (u16, Option<&str>), body: &[u8], sent: &str, token: &str) -> Option<ContactOutcome> {
    let result = ContactResult::of_status(op.kind, status)?;
    let mut outcome =
        ContactOutcome { op: op.clone(), result, status, retry_after: wait_of(retry_after, mailcraft_time::now()), ..Default::default() };
    let json = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let change_key = match text(&json, "changeKey") {
        "" => text(&json, "@odata.etag"),
        key => key,
    };
    match result {
        ContactResult::Done => {
            outcome.change_key = change_key.to_string();
            if op.kind == ContactChangeKind::Create {
                let id = text(&json, "id");
                outcome.remote = if valid_id(id) { id.to_string() } else { String::new() };
            }
        }
        ContactResult::Gone => {}
        ContactResult::Denied => outcome.error = "this account may not change these contacts".into(),
        ContactResult::Failed => {
            outcome.error = if status == 0 { "no answer".into() } else { format!("Microsoft Graph is busy or failed (status {status})") };
        }
        ContactResult::Refused => {
            let code = code_name(body, &[token, sent]).map(|code| format!(": {code}")).unwrap_or_default();
            outcome.error = format!("Microsoft Graph refused it (status {status}{code})");
        }
    }
    Some(outcome)
}

/// The names Graph has for the e-mail addresses of the contact at `item`, by address in lower
/// case; the status (and what it said to wait) when it couldn't be read. An answer that doesn't
/// list them is one that couldn't be read (status 0: tried again later), so that nothing goes up
/// that would rename them.
fn email_names(graph: &Graph<'_>, item: &str) -> Result<HashMap<String, String>, (u16, Option<String>, String)> {
    let url = graph.url(&format!("{item}?{}", http::form(&[("$select", "emailAddresses")])));
    let reply = graph
        .ask(&url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE)
        .map_err(|e| (if e.is_refused_sign_in() { 401u16 } else { 0 }, None, e.to_string()))?;
    if reply.status != 200 {
        return Err((reply.status, reply.retry_after, String::new()));
    }
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let Some(list) = json.get("emailAddresses").and_then(Value::as_array).filter(|l| l.iter().take(MAX_ENTRIES).all(Value::is_object)) else {
        return Err((0, None, "Microsoft Graph's answer didn't list the addresses".into()));
    };
    Ok(list
        .iter()
        .take(MAX_ENTRIES)
        .map(person)
        .filter(|a| !a.email.trim().is_empty() && !a.name.trim().is_empty())
        .map(|a| (a.email.trim().to_ascii_lowercase(), a.name))
        .collect())
}

/// What Graph is told of `values`: `fields`, and nothing else. `names` are the names an address
/// keeps; a new one is named after the contact.
fn contact_body(values: &RemoteContact, fields: &[ContactField], names: &HashMap<String, String>) -> Value {
    let mut body = Map::new();
    let mut put = |name: &str, value: Value| {
        body.insert(name.to_string(), value);
    };
    // (What has no place there isn't sent, and stays here: `keep_graph_overflow`.)
    let places = mailcraft_model::contact_sync::graph_places(&values.emails, &values.phones, &values.addresses, &values.websites);
    let address = |a: Option<&mailcraft_model::PostalAddress>| {
        let a = a.cloned().unwrap_or_default();
        json!({"street": a.street, "city": a.city, "state": a.region, "postalCode": a.postal_code, "countryOrRegion": a.country})
    };
    for field in fields {
        match field {
            ContactField::DisplayName => put("displayName", json!(values.display_name)),
            ContactField::First => put("givenName", json!(values.first)),
            ContactField::Middle => put("middleName", json!(values.middle)),
            ContactField::Last => put("surname", json!(values.last)),
            ContactField::Nickname => put("nickName", json!(values.nickname)),
            ContactField::Company => put("companyName", json!(values.company)),
            ContactField::Title => put("jobTitle", json!(values.title)),
            ContactField::Department => put("department", json!(values.department)),
            ContactField::Emails => {
                let own = if values.display_name.trim().is_empty() { None } else { Some(values.display_name.trim()) };
                let list: Vec<Value> = places
                    .emails
                    .iter()
                    .filter_map(|at| values.emails.get(*at))
                    .map(|e| {
                        let address = e.value.trim();
                        let name = names.get(&address.to_ascii_lowercase()).map(String::as_str).or(own).unwrap_or(address);
                        json!({"address": address, "name": name})
                    })
                    .collect();
                put("emailAddresses", Value::Array(list));
            }
            ContactField::Phones => {
                let number = |at: &usize| values.phones.get(*at).map(|p| json!(p.value.trim()));
                put("mobilePhone", places.mobile.as_ref().and_then(number).unwrap_or(json!("")));
                put("homePhones", Value::Array(places.home_phones.iter().filter_map(number).collect()));
                put("businessPhones", Value::Array(places.business_phones.iter().filter_map(number).collect()));
            }
            ContactField::Addresses => {
                let at = |place: Option<usize>| place.and_then(|at| values.addresses.get(at));
                put("businessAddress", address(at(places.business_address)));
                put("homeAddress", address(at(places.home_address)));
                put("otherAddress", address(at(places.other_address)));
            }
            ContactField::Websites => put("businessHomePage", json!(places.website.and_then(|at| values.websites.get(at)).map_or("", |w| w.trim()))),
            // (Noon UTC: the same day wherever it is read.)
            ContactField::Birthday => put("birthday", values.birthday.map_or(Value::Null, |day| json!(format!("{}T12:00:00Z", day.iso())))),
            ContactField::Notes => put("personalNotes", json!(values.notes)),
            ContactField::Categories => {
                let names: Vec<&str> = values.categories.iter().flatten().map(|c| c.trim()).filter(|c| !c.is_empty()).take(MAX_ENTRIES).collect();
                put("categories", json!(names));
            }
        }
    }
    Value::Object(body)
}
