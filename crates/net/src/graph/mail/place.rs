// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages made in a mailbox from another of the same owner's (#70): moved or copied between a
//! shared mailbox and its owner's, or between two of his shared mailboxes.
//!
//! Graph moves and copies a message only within its own mailbox, so what goes to another is made
//! there by that mailbox's own run, at its own address (`/users/{address}/…` for a shared mailbox,
//! `/me/…` for the owner's), from the whole message the engine sends ([`PlaceUp`]), one at a
//! time, after the changes of messages:
//!
//! 1. **Looked for** in the destination folder by MailCraft's marker (a make in an earlier run
//!    whose answer was lost). One found whole is the copy; one found less than whole is deleted
//!    and made again.
//! 2. **Made** with `POST …/mailFolders/{id}/messages`, its fields as JSON and, among its extended
//!    properties, the marker, its message flags (`PidTagMessageFlags`, read and not unsent: a
//!    message that was received, not a draft), the times it was received and sent
//!    (`PidTagMessageDeliveryTime`, `PidTagClientSubmitTime`) and its `Message-ID`
//!    (`PidTagInternetMessageId`). A make whose answer was lost is looked for again; not found,
//!    it waits for the next run. A make is never asked twice in a run.
//! 3. **Its attachments** added, as a draft's are (directly under 3 MB, else through an upload
//!    session).
//! 4. **Confirmed**: read back, it is in the folder, not a draft, with every attachment. Only then
//!    is it answered made, and only then does the engine let the original go (of a move). A copy
//!    that isn't whole, or that the server made a draft of, is deleted again and refused: the
//!    message stays where it was.
//!
//! A refusal (403, 400, a folder that is gone) is answered so, for the engine to put the message
//! back. In a shared mailbox a refusal by Graph's code of "no access" stops the run's places
//! (they wait); any other 403 holds that one here, counted. No subject, address, name or id of a
//! message goes into a log line or an error; what the server answers is hostile JSON.

use std::collections::HashMap;

use mailcraft_model::sync::{MAX_PLACES, MessageOutcome, PlaceDone, PlaceUp};
use mailcraft_model::{Address, Importance};
use mailcraft_time::{DateTime, Timestamp};
use serde_json::{Map, Value, json};

use super::super::pace::Kind;
use super::super::{Doing, Graph, MAX_PAGE, WRITE_TIMEOUT, code_of, text};
use super::id_of;
use super::up::MARKER;
use crate::NetError;
use crate::http::{self, Reply};

/// `PidTagMessageFlags`: written when a message is made, it makes one that was received (no
/// `MSGFLAG_UNSENT`), read or not (`MSGFLAG_READ`).
const MESSAGE_FLAGS: &str = "Integer 0x0E07";
/// `PidTagMessageDeliveryTime` and `PidTagClientSubmitTime`: when it was received, and sent.
const RECEIVED: &str = "SystemTime 0x0E06";
const SENT: &str = "SystemTime 0x0039";
/// `PidTagInternetMessageId`: its `Message-ID`.
const INTERNET_ID: &str = "String 0x1035";
/// Longest marker sent (the engine's are far shorter), longest `Message-ID`.
const MAX_MARKER: usize = 100;
const MAX_INTERNET_ID: usize = 250;
const MAX_RECIPIENTS: usize = 500;

/// What became of one message.
enum Step {
    /// Answered: made (and confirmed), or refused for good.
    Answered(MessageOutcome),
    /// It waits for the next run (no answer, a folder not on the server yet).
    Waits,
    /// A shared mailbox's 403 that isn't "no access": it waits here, counted.
    Held,
}

/// Make the messages of a run in their folders, putting what became of each into `out`. An
/// error is what stops the run's places (the server busy, no sign-in, no answer, a shared mailbox
/// that won't let the owner in). Answers how many a shared mailbox held back with a 403.
pub(super) fn places(graph: &Graph<'_>, made: &HashMap<u64, String>, places: &[PlaceUp], out: &mut Vec<PlaceDone>) -> Result<usize, NetError> {
    let mut held = 0usize;
    for place in places.iter().take(MAX_PLACES) {
        match one(graph, made, place) {
            Ok(Step::Answered(outcome)) => out.push(PlaceDone { change: place.without_data(), outcome }),
            Ok(Step::Waits) => {}
            Ok(Step::Held) => held = held.saturating_add(1),
            Err(e) if matches!(e, NetError::Busy { .. } | NetError::SharedRefused(_)) || e.is_refused_sign_in() || e.is_unreachable() => {
                return Err(e);
            }
            // This message's trouble (an answer that isn't one): the rest go on.
            Err(e) => log::info!("graph places: a message couldn't be put into its mailbox: {e}"),
        }
    }
    if !places.is_empty() {
        log::info!("graph places: {} of {} messages from another mailbox answered", out.len(), places.len().min(MAX_PLACES));
    }
    Ok(held)
}

fn marker_ok(marker: &str) -> bool {
    !marker.is_empty() && marker.len() <= MAX_MARKER && marker.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// An id as a piece of an address.
fn piece(id: &str) -> Option<String> {
    id_of(id).map(|id| http::encode(&id))
}

/// Answered for good with `status`, or held, or (no access) the error that stops the places.
fn refusal(graph: &Graph<'_>, reply: &Reply, what: &str) -> Result<Step, NetError> {
    super::refusing(graph, reply)?;
    let code = code_of(&reply.body, &[graph.token]);
    if graph.shared() && reply.status == 403 {
        log::info!("graph places: in a shared mailbox, {what} was refused (status 403{code}); it waits here");
        return Ok(Step::Held);
    }
    log::info!("graph places: {what} was refused (status {}{code})", reply.status);
    Ok(Step::Answered(MessageOutcome::Refused { status: reply.status }))
}

fn one(graph: &Graph<'_>, made: &HashMap<u64, String>, p: &PlaceUp) -> Result<Step, NetError> {
    // The folder isn't on the server yet: next time.
    let to = if p.to.is_empty() { p.to_folder.and_then(|f| made.get(&f).cloned()) } else { Some(p.to.clone()) };
    let Some(to) = to.and_then(|to| id_of(&to)) else { return Ok(Step::Waits) };
    if !marker_ok(&p.marker) {
        return Ok(Step::Answered(MessageOutcome::Refused { status: 400 }));
    }
    let wanted = p.attachments.len();
    // Made in an earlier run whose answer was lost: that one, when it is whole.
    if let Some(found) = look(graph, &to, &p.marker)? {
        match confirm(graph, &found, wanted)? {
            Confirmed::Whole(folder) => return Ok(Step::Answered(placed(folder, found))),
            Confirmed::Unread => return Ok(Step::Waits),
            Confirmed::Draft | Confirmed::Short => {
                log::info!("graph places: a copy made earlier wasn't whole; it is made again");
                remove(graph, &found)?;
            }
        }
    }
    let url = graph.url(&graph.mail_path(&format!("/mailFolders/{}/messages", http::encode(&to))));
    let body = Value::Object(fields(p)).to_string();
    let reply = match graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE) {
        Ok(reply) => reply,
        Err(e) if e.is_unreachable() => {
            log::info!("graph places: the answer to making a message was lost; it is looked for by its marker next run");
            return Ok(Step::Waits);
        }
        Err(e) => return Err(e),
    };
    let id = match reply.status {
        200 | 201 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            match id_of(text(&json, "id")) {
                Some(id) => id,
                // Made, but which one isn't said: found by its marker next run.
                None => return Ok(Step::Waits),
            }
        }
        400 | 403 | 404 | 410 | 413 => return refusal(graph, &reply, "making a message from another mailbox"),
        _ => return Err(graph.refusal(Doing::Mail, &reply, "")),
    };
    let Some(id_piece) = piece(&id) else { return Ok(Step::Answered(MessageOutcome::Refused { status: 400 })) };
    if wanted > 0 {
        let whole = super::drafts::attachments(graph, &id_piece, &p.attachments, &[])?.is_some_and(|list| list.len() == wanted);
        if !whole {
            log::info!("graph places: an attachment wasn't taken; the copy goes again and the message stays where it was");
            remove(graph, &id)?;
            return Ok(Step::Answered(MessageOutcome::Refused { status: 400 }));
        }
    }
    match confirm(graph, &id, wanted)? {
        Confirmed::Whole(folder) => Ok(Step::Answered(placed(folder, id))),
        // Read back next run, by its marker.
        Confirmed::Unread => Ok(Step::Waits),
        Confirmed::Draft | Confirmed::Short => {
            log::info!("graph places: the server didn't keep the copy as received mail; it goes again and the message stays where it was");
            remove(graph, &id)?;
            Ok(Step::Answered(MessageOutcome::Refused { status: 400 }))
        }
    }
}

/// Done: the copy is `id`, in `folder`.
fn placed(folder: String, id: String) -> MessageOutcome {
    MessageOutcome::Done { categories: None, flag: None, folder, id }
}

/// The message in the folder `to` that carries `marker`, when there is one.
fn look(graph: &Graph<'_>, to: &str, marker: &str) -> Result<Option<String>, NetError> {
    let filter = format!("singleValueExtendedProperties/any(ep: ep/id eq '{MARKER}' and ep/value eq '{marker}')");
    let query = http::form(&[("$filter", &filter), ("$select", "id"), ("$top", "1")]);
    let url = graph.url(&format!("{}?{query}", graph.mail_path(&format!("/mailFolders/{}/messages", http::encode(to)))));
    let reply = graph.call(Kind::Read, &url, "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    match reply.status {
        200 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            Ok(json.get("value").and_then(Value::as_array).and_then(|all| all.first()).and_then(|m| id_of(text(m, "id"))))
        }
        // The folder is gone, or not to be read: making it says what is so.
        400 | 403 | 404 | 410 => Ok(None),
        _ => Err(graph.refusal(Doing::Mail, &reply, "")),
    }
}

/// What a copy read back is.
enum Confirmed {
    /// In its folder (this one), received mail, with every attachment.
    Whole(String),
    /// A draft: the server didn't take it as received mail.
    Draft,
    /// Fewer attachments than it should have.
    Short,
    /// Not read (gone meanwhile, or no answer that says).
    Unread,
}

fn confirm(graph: &Graph<'_>, id: &str, wanted: usize) -> Result<Confirmed, NetError> {
    let Some(id_piece) = piece(id) else { return Ok(Confirmed::Unread) };
    let query = http::form(&[("$select", "id,parentFolderId,isDraft")]);
    let url = graph.url(&format!("{}?{query}", graph.mail_path(&format!("/messages/{id_piece}"))));
    let reply = graph.call(Kind::Read, &url, "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    if reply.status != 200 {
        return Ok(Confirmed::Unread);
    }
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    if json.get("isDraft").and_then(Value::as_bool) != Some(false) {
        return Ok(Confirmed::Draft);
    }
    let Some(folder) = id_of(text(&json, "parentFolderId")) else { return Ok(Confirmed::Unread) };
    if wanted > 0 && super::drafts::listed(graph, &id_piece)?.len() < wanted {
        return Ok(Confirmed::Short);
    }
    Ok(Confirmed::Whole(folder))
}

/// Delete a copy that isn't whole (Graph's delete: recoverable for the retention period).
fn remove(graph: &Graph<'_>, id: &str) -> Result<(), NetError> {
    let Some(id) = piece(id) else { return Ok(()) };
    let url = graph.url(&graph.mail_path(&format!("/messages/{id}")));
    let reply = graph.call(Kind::Same, &url, "DELETE", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    match reply.status {
        200..=299 | 404 | 410 => Ok(()),
        400 | 403 => {
            let code = code_of(&reply.body, &[graph.token]);
            log::info!("graph places: a copy that isn't whole couldn't be deleted (status {}{code})", reply.status);
            Ok(())
        }
        _ => Err(graph.refusal(Doing::Mail, &reply, "")),
    }
}

fn clean(text: &str, most: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(most).collect()
}

fn address(a: &Address) -> Value {
    json!({"emailAddress": {"address": clean(a.email.trim(), 320), "name": clean(a.name.trim(), 320)}})
}

fn recipients(list: &[Address]) -> Value {
    Value::Array(list.iter().filter(|a| !a.email.trim().is_empty()).take(MAX_RECIPIENTS).map(address).collect())
}

/// `2026-10-15T16:00:00Z`.
fn instant(at: Timestamp) -> String {
    format!("{}Z", DateTime::from_timestamp(at, 0).iso())
}

/// The message as Graph writes one that is made: its fields, and the extended properties that
/// make it received mail with its own times and `Message-ID`, carrying the marker.
fn fields(p: &PlaceUp) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("subject".into(), json!(clean(&p.subject, 4096)));
    out.insert("body".into(), json!({"contentType": if p.body.html { "html" } else { "text" }, "content": p.body.content}));
    if !p.from.email.trim().is_empty() {
        out.insert("from".into(), address(&p.from));
        out.insert("sender".into(), address(&p.from));
    }
    for (name, list) in [("toRecipients", &p.to_list), ("ccRecipients", &p.cc), ("replyTo", &p.reply_to)] {
        if !list.is_empty() {
            out.insert(name.into(), recipients(list));
        }
    }
    let level = match p.importance {
        Importance::Low => "low",
        Importance::Normal => "normal",
        Importance::High => "high",
    };
    out.insert("importance".into(), json!(level));
    out.insert("isRead".into(), json!(p.read));
    if !p.categories.is_empty() {
        out.insert("categories".into(), json!(p.categories.iter().take(25).map(|c| clean(c, 255)).collect::<Vec<_>>()));
    }
    if let Some(flag) = &p.flag {
        out.insert("flag".into(), super::up::flag_body(flag));
    }
    let mut extended = vec![
        json!({"id": MARKER, "value": p.marker}),
        json!({"id": MESSAGE_FLAGS, "value": if p.read { "1" } else { "0" }}),
        json!({"id": RECEIVED, "value": instant(p.received)}),
        json!({"id": SENT, "value": instant(p.date)}),
    ];
    let internet_id = p.message_id.trim().trim_start_matches('<').trim_end_matches('>');
    if !internet_id.is_empty() && internet_id.len() <= MAX_INTERNET_ID && internet_id.bytes().all(|b| b.is_ascii_graphic() && b != b'<' && b != b'>')
    {
        extended.push(json!({"id": INTERNET_ID, "value": format!("<{internet_id}>")}));
    }
    out.insert("singleValueExtendedProperties".into(), Value::Array(extended));
    out
}
