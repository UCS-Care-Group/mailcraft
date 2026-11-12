// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's drafts up through Graph (R3.4.1; R3 spec S23, S24).
//!
//! After the changes of messages, each draft the engine names ([`DraftUp`]) is brought up to
//! date on the server, one at a time:
//!
//! - **Made** when it has no server id yet: first looked for in Drafts by MailCraft's marker (a
//!   make in an earlier run whose answer was lost), and only when nothing has it made: with
//!   `POST /me/messages` and every field, or, for an answer or a forward of a message the server
//!   has, with `createReply`, `createReplyAll` or `createForward` (so that it stays in that
//!   message's conversation), the marker given with it and the fields changed after. A make
//!   whose answer was lost is looked for again before anything else is done; not found, it waits
//!   for the next run. A make is never asked twice in a run.
//! - **Changed in place** (`PATCH /me/messages/{id}`) in the fields that differ from what the
//!   server last had: subject, body, To, Cc, Bcc, Reply-To, importance.
//! - **Its attachments** brought in step with those here: the server's are listed, those it
//!   should keep are matched by what was put there before (or, for those that came from
//!   elsewhere, a copy of a forwarded message's, by name, type **and content**: a name alone
//!   says nothing of what a file holds), the rest are deleted, and what is missing is added: directly under
//!   3 MB, else through an upload session, in pieces that carry no sign-in. An attachment whose
//!   answer was lost is found by the next listing, so nothing is added twice.
//! - **Its change key** is read last: what tells an edit made elsewhere from this one (S26).
//!
//! A shared mailbox's drafts (#70) are made, changed and looked for at its own address
//! (`/users/{address}/messages`, its own Drafts), never the owner's; one that won't let the owner
//! in, by Graph's code, stops its drafts, which wait here.
//!
//! `In-Reply-To` and `References` can't be written by a client (Graph takes only `x-` headers in
//! `internetMessageHeaders`): an answer has them from create-reply. No subject, address, name or
//! id of a draft goes into a log line or an error; what the server answers is hostile JSON.

use mailcraft_charset::base64::STANDARD;
use mailcraft_model::sync::{AttachmentUp, DraftDone, DraftKind, DraftUp, MAX_DRAFTS, SyncedAttachment};
use mailcraft_model::{Address, Importance};
use serde_json::{Map, Value, json};

use super::super::pace::Kind;
use super::super::{Doing, Graph, MAX_PAGE, SEND_TIMEOUT, WRITE_TIMEOUT, code_of, text};
use super::id_of;
use super::up::MARKER;
use crate::NetError;
use crate::http::{self, Reply, Url};

/// An attachment smaller than this is added directly; a larger one through an upload session.
pub(crate) const DIRECT_LIMIT: u64 = 3 * 1024 * 1024;
/// The bytes of one piece of an upload (ten times 320 KiB, under Graph's 4 MB).
const PIECE: usize = 10 * 320 * 1024;
/// The largest attachment taken up (Exchange Online's limit of a message).
const MAX_ATTACHMENT: u64 = 150 * 1024 * 1024;
/// Most attachments of a draft listed, most recipients sent, longest marker.
const MAX_LISTED: usize = 500;
const MAX_RECIPIENTS: usize = 500;
const MAX_MARKER: usize = 100;

/// Where the messages of the mailbox are: `/v1.0/me/messages`, or a shared mailbox's own
/// (`/v1.0/users/{address}/messages`, #70). A draft written in a shared mailbox is made, changed
/// and sent there and nowhere else.
fn messages(graph: &Graph<'_>) -> String {
    graph.mail_path("/messages")
}

/// What making a draft came to.
enum Made {
    /// The server's id, the folder it is in, and whether the fields went with it.
    Id(String, String, bool),
    /// No answer: it may have been made.
    Lost,
    Refused(u16),
}

/// Bring the drafts of a run up to date on the server, putting what became of each into `out`.
/// A draft that got no answer at all isn't there. An error is what stops the run's drafts (the
/// server busy, no sign-in, no answer); what was done of the draft in hand is in `out` all the
/// same.
pub(super) fn drafts(graph: &Graph<'_>, drafts: &[DraftUp], out: &mut Vec<DraftDone>) -> Result<(), NetError> {
    for draft in drafts.iter().take(MAX_DRAFTS) {
        let mut done = DraftDone { change: draft.without_data(), id: draft.id.clone(), ..Default::default() };
        let result = one(graph, draft, &mut done);
        let answered = !done.id.is_empty() || done.refused.is_some() || done.conflict || done.purged;
        match result {
            Ok(()) if answered => out.push(done),
            Ok(()) => {}
            // (And a shared mailbox that won't let the owner in, #70: its drafts wait here.)
            Err(e) if matches!(e, NetError::Busy { .. } | NetError::SharedRefused(_)) || e.is_refused_sign_in() || e.is_unreachable() => {
                if answered {
                    out.push(done);
                }
                return Err(e);
            }
            // This draft's trouble (an answer that isn't one): the rest go on.
            Err(e) => {
                log::info!("graph drafts: a draft couldn't be taken up all the way: {e}");
                if answered {
                    out.push(done);
                }
            }
        }
    }
    if !drafts.is_empty() {
        log::info!("graph drafts: {} of {} drafts answered", out.len(), drafts.len().min(MAX_DRAFTS));
    }
    Ok(())
}

fn marker_ok(marker: &str) -> bool {
    !marker.is_empty() && marker.len() <= MAX_MARKER && marker.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// An id as a piece of an address.
fn piece(id: &str) -> Option<String> {
    id_of(id).map(|id| http::encode(&id))
}

pub(super) fn one(graph: &Graph<'_>, d: &DraftUp, done: &mut DraftDone) -> Result<(), NetError> {
    if !marker_ok(&d.marker) {
        done.refused = Some(400);
        return Ok(());
    }
    if d.purge_only || d.is_sealed() {
        // The copy the server had when this was saved: changed elsewhere since (an edit in another
        // program, or on another computer), or not known to be as it was, it is kept beside this one and
        // nothing is taken away (S26). One that came down from elsewhere has no marker of ours: it is
        // given it, so that it is found with the rest.
        match earlier_copy(graph, d)? {
            Earlier::Changed => {
                log::info!("graph drafts: a draft was changed elsewhere too, or isn't known to be as it was; nothing of it is taken away");
                done.conflict = true;
                return Ok(());
            }
            Earlier::Draft if d.purge => mark_earlier(graph, d)?,
            Earlier::Draft | Earlier::Leave => {}
        }
        if d.purge_only {
            return purge_copies(graph, &d.marker, "", done);
        }
        return sealed(graph, d, done);
    }
    let mut fields_went = false;
    if done.id.is_empty() {
        match look(graph, &d.marker)? {
            Some((id, folder)) => (done.id, done.folder) = (id, folder),
            None => match make(graph, d)? {
                Made::Id(id, folder, with_fields) => (done.id, done.folder, fields_went) = (id, folder, with_fields),
                Made::Lost => match look(graph, &d.marker)? {
                    Some((id, folder)) => (done.id, done.folder) = (id, folder),
                    // Not there: asked again next run, looked for first.
                    None => return Ok(()),
                },
                Made::Refused(status) => {
                    done.refused = Some(status);
                    return Ok(());
                }
            },
        }
    }
    let Some(id) = piece(&done.id) else {
        done.refused = Some(400);
        done.id.clear();
        return Ok(());
    };
    // Changed elsewhere since it was last seen: not written over (S26).
    if !d.id.is_empty() && !d.change_key.is_empty() {
        let query = http::form(&[("$select", "changeKey")]);
        let reply = graph.call(Kind::Read, &graph.url(&format!("{}/{id}?{query}", messages(graph))), "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
        super::refusing(graph, &reply)?;
        match reply.status {
            200 => {
                let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                let now = change_key(&json);
                if !now.is_empty() && now != d.change_key {
                    log::info!("graph drafts: a draft was changed elsewhere too; it isn't written over");
                    done.conflict = true;
                    return Ok(());
                }
            }
            404 | 410 | 400 | 403 => {
                refused(graph, &reply, "reading a draft");
                done.refused = Some(reply.status);
                return Ok(());
            }
            _ => return Err(graph.refusal(Doing::Mail, &reply, "")),
        }
    }
    if fields_went || !d.has_fields() {
        done.fields = true;
    } else {
        let body = Value::Object(fields(d)).to_string();
        let reply = graph.call(
            Kind::Same,
            &graph.url(&format!("{}/{id}", messages(graph))),
            "PATCH",
            &[("Content-Type", "application/json")],
            Some(&body),
            WRITE_TIMEOUT,
            MAX_PAGE,
        )?;
        super::refusing(graph, &reply)?;
        match reply.status {
            200..=299 => done.fields = true,
            404 | 410 | 400 | 403 => {
                refused(graph, &reply, "a change of a draft");
                done.refused = Some(reply.status);
                return Ok(());
            }
            _ => return Err(graph.refusal(Doing::Mail, &reply, &body)),
        }
    }
    if let Some(up) = &d.attachments {
        done.attachments = attachments(graph, &id, &up.here, &up.synced)?;
    }
    // Its change key after all of it.
    let query = http::form(&[("$select", "changeKey,parentFolderId")]);
    let reply = graph.call(Kind::Read, &graph.url(&format!("{}/{id}?{query}", messages(graph))), "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    if reply.status == 200 {
        let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        done.change_key = change_key(&json);
        if let Some(folder) = id_of(text(&json, "parentFolderId")) {
            done.folder = folder;
        }
    }
    Ok(())
}

/// What the server has of the copy `d.id` that an Encrypted save is to take the place of.
enum Earlier {
    /// None is known, or it is gone, or it is no draft any more (sent from elsewhere): left alone.
    Leave,
    /// It is a draft whose change key is the one last seen.
    Draft,
    /// Changed elsewhere since, or not known to be as it was: kept, and nothing is taken away.
    Changed,
}

/// Look at the copy `d.id`, just before the Encrypted draft is made and the earlier copies go. Fails
/// closed: a change key that isn't known here or there, or a read that failed (but "not found"), means
/// the copy may hold somebody's edit, and it is `Changed`. (A few milliseconds lie between this read and
/// the delete that follows; nothing here can close them.)
fn earlier_copy(graph: &Graph<'_>, d: &DraftUp) -> Result<Earlier, NetError> {
    let Some(at) = piece(&d.id) else { return Ok(Earlier::Leave) };
    let query = http::form(&[("$select", "changeKey,isDraft")]);
    let reply = graph.call(Kind::Read, &graph.url(&format!("{}/{at}?{query}", messages(graph))), "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    match reply.status {
        200 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            let now = change_key(&json);
            if d.change_key.is_empty() || now.is_empty() || now != d.change_key {
                return Ok(Earlier::Changed);
            }
            Ok(if json.get("isDraft").and_then(Value::as_bool) == Some(true) { Earlier::Draft } else { Earlier::Leave })
        }
        404 | 410 => Ok(Earlier::Leave),
        // (Not read: not known to be as it was.)
        _ => Ok(Earlier::Changed),
    }
}

/// Put the marker on the earlier copy, which has none when the draft was begun elsewhere, so that the
/// purge by marker (which lists Drafts, and checks each is a draft) finds it. Best effort: a copy that
/// can't be marked stays, and is said in the log.
fn mark_earlier(graph: &Graph<'_>, d: &DraftUp) -> Result<(), NetError> {
    let Some(at) = piece(&d.id) else { return Ok(()) };
    let body = json!({"singleValueExtendedProperties": [{"id": MARKER, "value": d.marker}]}).to_string();
    let url = graph.url(&format!("{}/{at}", messages(graph)));
    match graph.call(Kind::Same, &url, "PATCH", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE) {
        Ok(reply) if matches!(reply.status, 200..=299 | 404 | 410) => Ok(()),
        Ok(reply) => {
            refused(graph, &reply, "marking an earlier copy of a draft");
            Ok(())
        }
        Err(e) if e.is_unreachable() => Ok(()),
        Err(e) => Err(e),
    }
}

/// A draft kept Encrypted (#30 part 7): made on the server from its MIME, which is all that goes up
/// of it (`POST /me/messages` with `text/plain`, the message in base64: Microsoft's "Create message
/// in MIME format"), and the server's earlier copy of the draft, its fields in the clear, taken away
/// once the new one is there (nothing is lost if the new one fails). A draft made from MIME can't be
/// changed field by field, so a draft saved again is made again the same way. The marker, which a
/// MIME create can't carry, is put on it by a change right after.
///
/// An answer lost on the way leaves a draft this run can't find (it has no marker yet): the next run
/// makes another, and the owner may find the first beside it.
fn sealed(graph: &Graph<'_>, d: &DraftUp, done: &mut DraftDone) -> Result<(), NetError> {
    // Microsoft takes an S/MIME message made this way up to 4 MB: a larger one is refused (413) and
    // stays here as a draft of this computer's.
    let body = STANDARD.encode(d.sealed.as_slice());
    let url = graph.url(&messages(graph));
    let reply = match graph.call(Kind::Once, &url, "POST", &[("Content-Type", "text/plain")], Some(&body), WRITE_TIMEOUT, MAX_PAGE) {
        Ok(reply) => reply,
        Err(e) if e.is_unreachable() => {
            log::info!("graph drafts: the answer to making an Encrypted draft was lost; it is made again next run");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    super::refusing(graph, &reply)?;
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    match reply.status {
        200 | 201 => {}
        400 | 403 | 404 | 410 | 413 => {
            refused(graph, &reply, "making an Encrypted draft");
            done.refused = Some(reply.status);
            // The draft is kept here alone, then: an earlier copy of it on the server, which may hold
            // the text in the clear, goes all the same.
            if d.purge || !d.id.is_empty() {
                purge_copies(graph, &d.marker, "", done)?;
            }
            return Ok(());
        }
        // (What was sent is never put in the log.)
        _ => return Err(graph.refusal(Doing::Mail, &reply, "")),
    }
    let Some(new) = id_of(text(&json, "id")) else {
        // Made, but which one isn't said: it can't be found, so it is made again next run.
        return Ok(());
    };
    done.id = new.clone();
    done.folder = id_of(text(&json, "parentFolderId")).unwrap_or_default();
    done.fields = true;
    let Some(at) = piece(&new) else { return Ok(()) };
    // The marker (which draft of this computer's it is), best effort: the draft is there without it.
    let marker = json!({"singleValueExtendedProperties": [{"id": MARKER, "value": d.marker}]}).to_string();
    let url = graph.url(&format!("{}/{at}", messages(graph)));
    match graph.call(Kind::Same, &url, "PATCH", &[("Content-Type", "application/json")], Some(&marker), WRITE_TIMEOUT, MAX_PAGE) {
        Ok(reply) if matches!(reply.status, 200..=299) => {}
        Ok(reply) => refused(graph, &reply, "marking an Encrypted draft"),
        Err(e) if e.is_unreachable() => {}
        Err(e) => return Err(e),
    }
    // The earlier copies go for good: they may hold the text in the clear, and a delete would
    // leave them in the recoverable items. Found by the marker and checked first.
    if d.purge || !d.id.is_empty() {
        purge_copies(graph, &d.marker, &new, done)?;
    }
    // An earlier try whose answer was lost left its draft with no marker: found by the Message-ID
    // every copy of this message carries, checked, and taken away too. Best effort.
    // (Only by a Message-ID this program wrote for this draft: never by the message's own, which other
    // drafts of the owner's can share.)
    if let Some(filter) =
        message_id_in(&d.sealed).filter(|id| mailcraft_model::encrypting::is_sealed_draft_message_id(id)).as_deref().and_then(by_internet_id)
        && let Err(e) = purge_matching(graph, &filter, &new)
    {
        log::info!("graph drafts: copies of an Encrypted draft made before couldn't be looked for ({e})");
    }
    // Its change key and folder as they are now.
    let query = http::form(&[("$select", "changeKey,parentFolderId")]);
    if let Ok(reply) = graph.call(Kind::Read, &graph.url(&format!("{}/{at}?{query}", messages(graph))), "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)
        && reply.status == 200
    {
        let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        done.change_key = change_key(&json);
        if let Some(folder) = id_of(text(&json, "parentFolderId")) {
            done.folder = folder;
        }
    }
    Ok(())
}

/// A draft a listing found.
struct Found {
    id: String,
    internet_id: String,
    /// When it was made, if the server said.
    created: Option<mailcraft_time::Timestamp>,
}

/// How many pages of a list of drafts are read (fifty each) before it is said to be unfinished.
const MAX_LIST_PAGES: usize = 20;

/// How old a Bcc copy's draft is before it is taken for one left behind.
const ORPHAN_AFTER_SECONDS: i64 = 15 * 60;

/// The marker of the drafts the copies for Bcc recipients are made as (every one the same).
pub(super) const BCC_COPY_MARKER: &str = "mailcraft-bcc-copy";

/// The filter for the messages with `marker`.
fn marker_filter(marker: &str) -> String {
    format!("singleValueExtendedProperties/any(ep: ep/id eq '{MARKER}' and ep/value eq '{marker}')")
}

/// The filter for the messages that carry the Message-ID `id` (a Message-ID this program made:
/// anything else is not looked for).
pub(super) fn by_internet_id(id: &str) -> Option<String> {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    let plain = !id.is_empty() && id.len() <= 200 && id.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'\'' | b'<' | b'>' | b'"' | b'\\'));
    plain.then(|| format!("internetMessageId eq '<{id}>'"))
}

/// The drafts in Drafts that `filter` finds: the earlier copies of a draft, which are all there is to
/// take away. A message that is no draft (sent from another device since, which keeps its id), or is
/// not in Drafts (moved), is not among them, and is never touched. Whether the whole list was read
/// (it is followed page by page, within bounds) is said with it.
fn drafts_where(graph: &Graph<'_>, filter: &str) -> Result<(Vec<Found>, bool), NetError> {
    let query = http::form(&[("$filter", filter), ("$select", "id,isDraft,internetMessageId,createdDateTime"), ("$top", "50")]);
    let mut url = graph.url(&format!("{}?{query}", graph.mail_path("/mailFolders/drafts/messages")));
    let (mut drafts, mut left) = (Vec::new(), 0usize);
    let mut whole = false;
    for _ in 0..MAX_LIST_PAGES {
        let reply = graph.call(Kind::Read, &url, "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
        if reply.status != 200 {
            return Err(graph.refusal(Doing::Mail, &reply, ""));
        }
        let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        for m in json.get("value").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            match (m.get("isDraft").and_then(Value::as_bool), id_of(text(m, "id"))) {
                (Some(true), Some(id)) => drafts.push(Found {
                    id,
                    internet_id: text(m, "internetMessageId").to_string(),
                    created: super::super::instant(text(m, "createdDateTime")),
                }),
                _ => left += 1,
            }
        }
        match super::down::link_of(graph, &json, "@odata.nextLink") {
            Some((_, next)) => url = next,
            None => {
                // (A link that can't be followed is an unfinished list, not a finished one.)
                whole = json.get("@odata.nextLink").is_none_or(Value::is_null);
                break;
            }
        }
    }
    if left > 0 {
        log::info!("graph drafts: {left} messages found with a draft's marker are no draft any more and are left alone");
    }
    Ok((drafts, whole))
}

/// Whether the message `at` is a draft right now (asked just before it is taken away: a draft sent
/// from another device since the list was read keeps its id, and is no draft any more). There is a
/// window of a few milliseconds between this answer and the delete that nothing here can close.
fn still_a_draft(graph: &Graph<'_>, at: &str) -> Result<bool, NetError> {
    let query = http::form(&[("$select", "isDraft")]);
    let reply = graph.call(Kind::Read, &graph.url(&format!("{}/{at}?{query}", messages(graph))), "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    match reply.status {
        200 => Ok(serde_json::from_slice::<Value>(&reply.body).ok().and_then(|j| j.get("isDraft").and_then(Value::as_bool)) == Some(true)),
        404 => Ok(false),
        _ => Err(graph.refusal(Doing::Mail, &reply, "")),
    }
}

/// Take away, beyond recovery, every draft in Drafts that `filter` finds but `keep` (the one it has
/// now, or none), each checked to be a draft still just before. Whether the whole list was read.
pub(super) fn purge_matching(graph: &Graph<'_>, filter: &str, keep: &str) -> Result<bool, NetError> {
    let keep = id_of(keep).unwrap_or_default();
    let (found, whole) = drafts_where(graph, filter)?;
    for Found { id, .. } in found {
        if id == keep {
            continue;
        }
        let Some(at) = piece(&id) else { continue };
        if !still_a_draft(graph, &at)? {
            log::info!("graph drafts: a message found with a draft's marker is no draft any more and is left alone");
            continue;
        }
        remove_for_good(graph, &at)?;
    }
    Ok(whole)
}

/// Take away, beyond recovery, the drafts the copies for Bcc recipients left behind (a send that was
/// stopped, the owner's message deleted from the Outbox since) whose message is not in `keep` (the
/// Message-IDs of what waits in the Outbox). Found by the marker they carry and a Message-ID of the
/// shape only this program makes; each checked to be a draft, in Drafts, just before. One listing,
/// within bounds; what is left is looked for by the next run.
pub(super) fn purge_orphan_copies(graph: &Graph<'_>, keep: &[String]) -> Result<(), NetError> {
    let keep: Vec<String> = keep.iter().map(|id| format!("<{}>", id.trim().trim_start_matches('<').trim_end_matches('>'))).collect();
    let (found, _) = drafts_where(graph, &marker_filter(BCC_COPY_MARKER))?;
    // (Not those made in the last quarter of an hour: another computer of the owner's may be sending
    // one this moment, and its Outbox is not this one's.)
    let before = mailcraft_time::now().saturating_sub(ORPHAN_AFTER_SECONDS);
    for Found { id, internet_id, created } in found {
        if created.is_none_or(|at| at > before) {
            continue;
        }
        let Some(base) = mailcraft_model::encrypting::blind_copy_base_message_id(&internet_id) else { continue };
        if keep.contains(&base) {
            continue;
        }
        let Some(at) = piece(&id) else { continue };
        if !still_a_draft(graph, &at)? {
            continue;
        }
        remove_for_good(graph, &at)?;
    }
    Ok(())
}

/// Take away every earlier copy of the draft marked `marker` but `keep`: `done.purged` says they
/// were dealt with, which is when the whole list of them was read.
fn purge_copies(graph: &Graph<'_>, marker: &str, keep: &str, done: &mut DraftDone) -> Result<(), NetError> {
    if !marker_ok(marker) {
        return Ok(());
    }
    done.purged = purge_matching(graph, &marker_filter(marker), keep)?;
    Ok(())
}

/// The Message-ID of a message written out (its header, up to the first blank line).
fn message_id_in(bytes: &[u8]) -> Option<String> {
    let head = bytes.split(|b| *b == b'\n').take(200).map(|l| String::from_utf8_lossy(l).trim_end().to_string());
    for line in head {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("message-id")
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Take a draft away so that it can't be had back: Microsoft's permanent delete of a message (it goes
/// to the purges folder, which no mail client reads, for the retention period alone). Only a mailbox
/// that doesn't offer it (400, 403, 405, 501) gets the plain `DELETE`, which keeps the copy
/// recoverable for the retention period; that is logged, without the id. Any other answer (the
/// server busy, throttled) is an error for the run, which tries again with the next: a copy that may
/// hold the text in the clear is never soft-deleted for that.
fn remove_for_good(graph: &Graph<'_>, id: &str) -> Result<(), NetError> {
    let url = graph.url(&format!("{}/{id}/permanentDelete", messages(graph)));
    let reply = graph.call(Kind::Same, &url, "POST", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    match reply.status {
        200..=299 | 404 | 410 => return Ok(()),
        400 | 403 | 405 | 501 => log::info!("graph drafts: a permanent delete isn't offered (status {}); the plain delete is used", reply.status),
        _ => return Err(graph.refusal(Doing::Mail, &reply, "")),
    }
    let url = graph.url(&format!("{}/{id}", messages(graph)));
    let reply = graph.call(Kind::Same, &url, "DELETE", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    match reply.status {
        200..=299 | 404 | 410 => Ok(()),
        400 | 403 => {
            refused(graph, &reply, "taking away an earlier copy of a draft");
            Ok(())
        }
        _ => Err(graph.refusal(Doing::Mail, &reply, "")),
    }
}

fn change_key(json: &Value) -> String {
    Some(text(json, "changeKey")).filter(|key| key.len() <= 512 && key.bytes().all(|b| b.is_ascii_graphic())).map(str::to_string).unwrap_or_default()
}

fn refused(graph: &Graph<'_>, reply: &Reply, what: &str) {
    let code = code_of(&reply.body, &[graph.token]);
    log::info!("graph drafts: {what} was refused (status {}{code})", reply.status);
}

/// The draft in Drafts that carries `marker`, when there is one: its id and folder.
fn look(graph: &Graph<'_>, marker: &str) -> Result<Option<(String, String)>, NetError> {
    let filter = format!("singleValueExtendedProperties/any(ep: ep/id eq '{MARKER}' and ep/value eq '{marker}')");
    let query = http::form(&[("$filter", &filter), ("$select", "id,parentFolderId"), ("$top", "1")]);
    let reply = graph.call(
        Kind::Read,
        &graph.url(&format!("{}?{query}", graph.mail_path("/mailFolders/drafts/messages"))),
        "GET",
        &[],
        None,
        WRITE_TIMEOUT,
        MAX_PAGE,
    )?;
    if reply.status != 200 {
        return Err(graph.refusal(Doing::Mail, &reply, ""));
    }
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let first = json.get("value").and_then(Value::as_array).and_then(|all| all.first());
    Ok(first.and_then(|m| Some((id_of(text(m, "id"))?, id_of(text(m, "parentFolderId")).unwrap_or_default()))))
}

/// Make the draft: as an answer or a forward when it is one, else from its fields.
fn make(graph: &Graph<'_>, d: &DraftUp) -> Result<Made, NetError> {
    let marker = json!([{"id": MARKER, "value": d.marker}]);
    if let Some(of) = &d.of
        && let Some(source) = piece(&of.id)
    {
        let action = match of.kind {
            DraftKind::Reply => "createReply",
            DraftKind::ReplyAll => "createReplyAll",
            DraftKind::Forward => "createForward",
        };
        let body = json!({"message": {"singleValueExtendedProperties": marker}}).to_string();
        let url = graph.url(&format!("{}/{source}/{action}", messages(graph)));
        match made(
            graph,
            graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE),
            &body,
            false,
        )? {
            // The message it answers is gone: made as a message of its own.
            Made::Refused(404 | 410) => log::info!("graph drafts: the message a draft answers is gone; it is made by itself"),
            other => return Ok(other),
        }
    }
    let mut body = fields(d);
    body.insert("singleValueExtendedProperties".into(), marker);
    let body = Value::Object(body).to_string();
    let url = graph.url(&messages(graph));
    made(graph, graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE), &body, true)
}

fn made(graph: &Graph<'_>, asked: Result<Reply, NetError>, sent: &str, with_fields: bool) -> Result<Made, NetError> {
    let reply = match asked {
        Ok(reply) => reply,
        Err(e) if e.is_unreachable() => {
            log::info!("graph drafts: the answer to making a draft was lost; it is looked for by its marker");
            return Ok(Made::Lost);
        }
        Err(e) => return Err(e),
    };
    super::refusing(graph, &reply)?;
    match reply.status {
        200 | 201 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            match id_of(text(&json, "id")) {
                Some(id) => Ok(Made::Id(id, id_of(text(&json, "parentFolderId")).unwrap_or_default(), with_fields)),
                // Made, but which one isn't said: found by its marker.
                None => Ok(Made::Lost),
            }
        }
        400 | 403 | 404 | 410 => {
            refused(graph, &reply, "making a draft");
            Ok(Made::Refused(reply.status))
        }
        _ => Err(graph.refusal(Doing::Mail, &reply, sent)),
    }
}

fn recipients(list: &[Address]) -> Value {
    let clean = |text: &str| text.chars().filter(|c| !c.is_control()).take(320).collect::<String>();
    Value::Array(
        list.iter()
            .filter(|a| !a.email.trim().is_empty())
            .take(MAX_RECIPIENTS)
            .map(|a| json!({"emailAddress": {"address": clean(a.email.trim()), "name": clean(a.name.trim())}}))
            .collect(),
    )
}

/// The fields a draft names, as Graph writes a message.
fn fields(d: &DraftUp) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(subject) = &d.subject {
        out.insert("subject".into(), json!(subject.chars().filter(|c| !c.is_control()).collect::<String>()));
    }
    if let Some(body) = &d.body {
        out.insert("body".into(), json!({"contentType": if body.html { "html" } else { "text" }, "content": body.content}));
    }
    for (name, list) in [("toRecipients", &d.to), ("ccRecipients", &d.cc), ("bccRecipients", &d.bcc), ("replyTo", &d.reply_to)] {
        if let Some(list) = list {
            out.insert(name.into(), recipients(list));
        }
    }
    if let Some(importance) = d.importance {
        let level = match importance {
            Importance::Low => "low",
            Importance::Normal => "normal",
            Importance::High => "high",
        };
        out.insert("importance".into(), json!(level));
    }
    if let Some(read_receipt) = d.read_receipt {
        out.insert("isReadReceiptRequested".into(), json!(read_receipt));
    }
    out
}

/// An attachment the server has on a draft.
pub(super) struct There {
    id: String,
    name: String,
    /// The size the server states (it counts a little beyond the bytes).
    size: u64,
    content_type: String,
    inline: bool,
}

/// The attachments the server has on the draft `id`.
pub(super) fn listed(graph: &Graph<'_>, id: &str) -> Result<Vec<There>, NetError> {
    let query = http::form(&[("$select", "id,name,size,contentType,isInline"), ("$top", "100")]);
    let mut out = Vec::new();
    graph.list(Doing::Mail, graph.url(&format!("{}/{id}/attachments?{query}", messages(graph))), &[], MAX_LISTED, &mut |value| {
        if let Some(aid) = id_of(text(value, "id")) {
            out.push(There {
                id: aid,
                name: text(value, "name").chars().take(255).collect(),
                size: value.get("size").and_then(Value::as_u64).unwrap_or(u64::MAX),
                content_type: text(value, "contentType").chars().take(255).collect(),
                inline: value.get("isInline").and_then(Value::as_bool).unwrap_or(false),
            });
        }
    })?;
    Ok(out)
}

/// Bring the draft's attachments in step with `here`. The list the server has when every one of
/// `here` is there; `None` when some couldn't be put there (they wait for the next run).
pub(super) fn attachments(
    graph: &Graph<'_>,
    id: &str,
    here: &[AttachmentUp],
    synced: &[SyncedAttachment],
) -> Result<Option<Vec<SyncedAttachment>>, NetError> {
    let there = listed(graph, id)?;
    let mut have: Vec<Option<SyncedAttachment>> = vec![None; here.len()];
    let mut extra = Vec::new();
    let claim = |have: &mut Vec<Option<SyncedAttachment>>, aid: &str, fits: &dyn Fn(&AttachmentUp) -> bool| -> bool {
        let free = here.iter().zip(have.iter()).position(|(a, slot)| slot.is_none() && fits(a));
        match free.and_then(|at| here.get(at).map(|a| (at, a))) {
            Some((at, a)) => {
                if let Some(slot) = have.get_mut(at) {
                    *slot = Some(SyncedAttachment { id: aid.to_string(), name: a.name.clone(), hash: a.hash });
                }
                true
            }
            None => false,
        }
    };
    // Those put there before, by what they were; then those from elsewhere, by name.
    let (known, unknown): (Vec<&There>, Vec<&There>) = there.iter().partition(|t| synced.iter().any(|s| s.id == t.id));
    for t in known {
        let was = synced.iter().find(|s| s.id == t.id);
        let kept = was.is_some_and(|was| claim(&mut have, &t.id, &|a: &AttachmentUp| a.name == was.name && a.hash == was.hash));
        if !kept {
            extra.push(t.id.clone());
        }
    }
    // (Their bytes are read and compared: a file of the same name may be another file.)
    for t in unknown {
        let like = |a: &AttachmentUp| {
            a.name == t.name
                && a.inline == t.inline
                && (a.mime.is_empty() || t.content_type.is_empty() || a.mime.eq_ignore_ascii_case(&t.content_type))
        };
        let wanted = here.iter().zip(have.iter()).any(|(a, slot)| slot.is_none() && like(a));
        let bytes = if wanted { content_of(graph, id, t)? } else { None };
        let claimed = bytes.is_some_and(|bytes| claim(&mut have, &t.id, &|a: &AttachmentUp| like(a) && a.data.as_slice() == bytes.as_slice()));
        if !claimed {
            extra.push(t.id.clone());
        }
    }
    for aid in extra {
        let Some(aid) = piece(&aid) else { continue };
        let reply = graph.call(
            Kind::Same,
            &graph.url(&format!("{}/{id}/attachments/{aid}", messages(graph))),
            "DELETE",
            &[],
            None,
            WRITE_TIMEOUT,
            MAX_PAGE,
        )?;
        super::refusing(graph, &reply)?;
        match reply.status {
            200..=299 | 404 | 410 => {}
            400 | 403 => refused(graph, &reply, "removing an attachment"),
            _ => return Err(graph.refusal(Doing::Mail, &reply, "")),
        }
    }
    let mut whole = true;
    for (at, a) in here.iter().enumerate() {
        if have.get(at).is_some_and(Option::is_some) {
            continue;
        }
        if a.size > MAX_ATTACHMENT || a.data.len() as u64 != a.size {
            log::info!("graph drafts: an attachment is too large for the server, or isn't whole here; it isn't taken up");
            whole = false;
            continue;
        }
        let added = if a.size < DIRECT_LIMIT { add(graph, id, a)? } else { upload(graph, id, a)? };
        let added = match added {
            Added::Id(aid) => Some(aid),
            // Perhaps there after all: the next listing says.
            Added::Lost => {
                // One that was not there before this run's listing, so none of a forward's own.
                let taken: Vec<&str> = have.iter().flatten().map(|s| s.id.as_str()).collect();
                let before: Vec<&str> = there.iter().map(|t| t.id.as_str()).collect();
                listed(graph, id)?
                    .into_iter()
                    .find(|t| t.name == a.name && !taken.contains(&t.id.as_str()) && !before.contains(&t.id.as_str()))
                    .map(|t| t.id)
            }
            Added::Not => None,
        };
        match (added, have.get_mut(at)) {
            (Some(aid), Some(slot)) => *slot = Some(SyncedAttachment { id: aid, name: a.name.clone(), hash: a.hash }),
            _ => whole = false,
        }
    }
    Ok(whole.then(|| have.into_iter().flatten().collect()))
}

/// The most an attachment may be (by the size the server states) to be read back to be compared.
const COMPARE_LIMIT: u64 = 2800 * 1024;

/// The bytes of the server's attachment `t`, when they can be had and are not too many to read.
/// `None` is "can't tell": the attachment is then not taken for the owner's.
fn content_of(graph: &Graph<'_>, id: &str, t: &There) -> Result<Option<Vec<u8>>, NetError> {
    let Some(aid) = piece(&t.id).filter(|_| t.size <= COMPARE_LIMIT) else { return Ok(None) };
    let url = graph.url(&format!("{}/{id}/attachments/{aid}", messages(graph)));
    let reply = graph.call(Kind::Read, &url, "GET", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    if reply.status != 200 {
        return Ok(None);
    }
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    Ok(STANDARD.decode(text(&json, "contentBytes")).ok())
}

/// What adding an attachment came to.
enum Added {
    Id(String),
    /// No answer, or none that says which it is.
    Lost,
    /// Refused.
    Not,
}

fn attachment_fields(a: &AttachmentUp) -> Map<String, Value> {
    let mut out = Map::new();
    let clean = |text: &str| text.chars().filter(|c| !c.is_control()).take(255).collect::<String>();
    out.insert("name".into(), json!(clean(&a.name)));
    if !a.mime.is_empty() {
        out.insert("contentType".into(), json!(clean(&a.mime)));
    }
    if a.inline {
        out.insert("isInline".into(), json!(true));
    }
    if !a.content_id.is_empty() {
        out.insert("contentId".into(), json!(clean(&a.content_id)));
    }
    out
}

/// `POST /me/messages/{id}/attachments`, the bytes in base64.
fn add(graph: &Graph<'_>, id: &str, a: &AttachmentUp) -> Result<Added, NetError> {
    let mut body = attachment_fields(a);
    body.insert("@odata.type".into(), json!("#microsoft.graph.fileAttachment"));
    body.insert("contentBytes".into(), json!(STANDARD.encode(a.data.as_slice())));
    let body = Value::Object(body).to_string();
    let url = graph.url(&format!("{}/{id}/attachments", messages(graph)));
    let reply = match graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some(&body), SEND_TIMEOUT, MAX_PAGE) {
        Ok(reply) => reply,
        Err(e) if e.is_unreachable() => return Ok(Added::Lost),
        Err(e) => return Err(e),
    };
    super::refusing(graph, &reply)?;
    match reply.status {
        200 | 201 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            Ok(id_of(text(&json, "id")).map_or(Added::Lost, Added::Id))
        }
        400 | 403 | 404 | 413 => {
            refused(graph, &reply, "adding an attachment");
            Ok(Added::Not)
        }
        _ => Err(graph.refusal(Doing::Mail, &reply, "")),
    }
}

/// Where an upload session says to put the bytes, when that is one of Microsoft's own addresses
/// over TLS (or the server spoken to already): nothing of the owner's goes anywhere else.
fn upload_address(graph: &Graph<'_>, text: &str) -> Option<Url> {
    let url = Url::parse_within(text, http::MAX_LONG_URL).ok()?;
    let host = url.host.to_ascii_lowercase();
    let microsoft = ["office.com", "office365.com", "outlook.com", "microsoft.com"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")));
    ((url.secure && microsoft) || graph.base.same_server(&url)).then_some(url)
}

/// An upload session for a large attachment, and its bytes in pieces.
fn upload(graph: &Graph<'_>, id: &str, a: &AttachmentUp) -> Result<Added, NetError> {
    let mut item = attachment_fields(a);
    item.insert("attachmentType".into(), json!("file"));
    item.insert("size".into(), json!(a.size));
    let body = json!({"AttachmentItem": Value::Object(item)}).to_string();
    let url = graph.url(&format!("{}/{id}/attachments/createUploadSession", messages(graph)));
    // A session makes nothing until it is filled: asking for another is harmless.
    let reply = graph.call(Kind::Same, &url, "POST", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    match reply.status {
        200 | 201 => {}
        400 | 403 | 404 | 413 => {
            refused(graph, &reply, "an upload of an attachment");
            return Ok(Added::Not);
        }
        _ => return Err(graph.refusal(Doing::Mail, &reply, "")),
    }
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let Some(to) = upload_address(graph, text(&json, "uploadUrl")) else {
        log::warn!("graph drafts: an upload session named an address that isn't Microsoft's; the attachment isn't sent there");
        return Ok(Added::Not);
    };
    let total = a.data.len();
    for (n, bytes) in a.data.chunks(PIECE).enumerate() {
        let from = n.saturating_mul(PIECE);
        let last = from.saturating_add(bytes.len()).saturating_sub(1);
        let range = format!("bytes {from}-{last}/{total}");
        let reply = match graph.upload(&to, &range, bytes) {
            Ok(reply) => reply,
            Err(e) if e.is_unreachable() => return Ok(Added::Lost),
            Err(e) => return Err(e),
        };
        match reply.status {
            200 | 202 if last + 1 < total => {}
            // The last piece: made. Which one it is, the listing says.
            200 | 201 => return Ok(Added::Lost),
            429 | 503 => return Err(graph.refusal(Doing::Mail, &reply, "")),
            _ => {
                refused(graph, &reply, "a piece of an upload");
                return Ok(Added::Not);
            }
        }
    }
    Ok(Added::Lost)
}
