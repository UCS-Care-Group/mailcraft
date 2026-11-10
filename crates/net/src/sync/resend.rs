// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mail Resend received for the owner's domain, read from Resend's API: Resend has no IMAP.
//! What is sent goes through Resend's SMTP server like any other account's ([`super::send_outbox`]).
//!
//! An account at Resend names [`RESEND_API_HOST`] as its incoming server and its API key as its
//! password. The key goes to that host alone, over TLS, as `Authorization: Bearer` ([`may_go_to`]),
//! and never in an address, an error or a log line. Two requests are made of it, as Resend's API
//! reference has them: `GET /emails/receiving` lists what came, newest first, a hundred at a time,
//! and `GET /emails/receiving/{id}` says where the message's bytes are; those are then fetched
//! from the signed address Resend gives (on a host under `resend.com`, over TLS), with nothing
//! of the owner's sent along.
//!
//! The account has one folder there, its Inbox ([`INBOX`]). Resend knows no flags, moves or
//! folders: what the owner changes here stays here, and the run says the change is done so that
//! it isn't asked again. Each message is given a UID made from when it came ([`numbered`]), so
//! that a run brings only what is newer than what the folder has.

use mailcraft_model::provider::RESEND_API_HOST;
use mailcraft_model::sync::{Deleted, FlagsPushed, FolderDelta, ListedFolder, Moved, RemoteOp, SyncStage, SyncStep};
use mailcraft_model::{Account, FolderKind};
use mailcraft_proto::ProtoError;
use serde_json::Value;
use std::time::Duration;

use super::{BATCH_MESSAGES, Run, state_for};
use crate::NetError;
use crate::http::{self, MAX_LONG_URL, Request, Url};

/// The one folder an account at Resend has on the server, as its path is kept.
const INBOX: &str = "INBOX";
/// The `UIDVALIDITY` the folder is kept with: the UIDs only ever mean what [`numbered`] says.
const UID_VALIDITY: u32 = 1;
/// UIDs count from this instant (2024-01-01T00:00:00Z)...
const BASE: i64 = 1_704_067_200;
/// ...in this many steps a second: messages that came in the same second are told apart by when
/// in it they came. shortcut: the UIDs last until 2041 and a ninth message in one second shares
/// the eighth's UID (and isn't brought); give the folder a new `UIDVALIDITY` and a later base then.
const SLOTS: i64 = 8;
/// Most messages listed in one run (100 a page): the newest, on a first run.
const MAX_PAGES: usize = 50;
/// How long Resend's API has to be reached and to answer one request.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The largest list or message description read.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// The longest API key shown to Resend.
const MAX_KEY: usize = 1024;

/// Whether the account's mail is read from Resend's API rather than IMAP.
pub(crate) fn is_resend(account: &Account) -> bool {
    account.incoming.as_ref().is_some_and(|server| mailcraft_model::provider::is_resend_host(&server.host))
}

/// Check that Resend takes the API key and lets it read received mail: one list of one message.
pub(crate) fn check(key: &str) -> Result<(), NetError> {
    get(key, "/emails/receiving?limit=1").map(|_| ())
}

/// The changes made here are said done (Resend has nothing to change), then what came since the
/// last run is brought down, oldest first, a batch at a time.
pub(super) fn exchange(run: &mut Run) {
    let request = run.request;
    settle_changes(run);
    if request.push_only || run.cancelled() {
        return;
    }
    run.hand_over(SyncStep::new(SyncStage::Connecting, "", 0, 0));
    run.out.listed = vec![ListedFolder { remote: INBOX.into(), kind: FolderKind::Inbox, parent: String::new(), name: "Inbox".into() }];
    let key = request.secret();
    let state = state_for(request, INBOX, FolderKind::Inbox);
    let seen = state.filter(|s| s.uid_validity == UID_VALIDITY).map_or(0, |s| s.uid_seen);
    let known: std::collections::HashSet<u32> = state.map(|s| s.known_uids.iter().copied().collect()).unwrap_or_default();
    let listed = match list_since(key, seen) {
        Ok(listed) => listed,
        Err(e) => {
            run.failed("Couldn't check for new mail", &e);
            return;
        }
    };
    let mut new: Vec<(u32, Listed)> = numbered(listed).into_iter().filter(|(uid, _)| *uid > seen && !known.contains(uid)).collect();
    // The newest so many, when the folder keeps no more.
    let most = usize::try_from(request.max_per_folder).unwrap_or(usize::MAX);
    if most > 0 && new.len() > most {
        new.drain(..new.len() - most);
    }
    let total = new.len();
    let blank = FolderDelta {
        id: state.map(|s| s.id),
        remote: INBOX.into(),
        kind: FolderKind::Inbox,
        uid_validity: UID_VALIDITY,
        uid_seen: seen,
        ..Default::default()
    };
    let mut delta = blank.clone();
    let mut whole = true;
    for (done, (uid, listed)) in new.into_iter().enumerate() {
        if run.cancelled() {
            whole = false;
            break;
        }
        run.asked = run.asked.saturating_add(2);
        match fetch(key, &listed.id, run.limits.largest) {
            Ok(bytes) => {
                let mut message = mailcraft_mime::parse_fetched(&bytes);
                message.uid = uid;
                message.received = listed.secs;
                delta.new.push(message);
            }
            // Never brought whole: said once, and not asked for again.
            Err(e) if http::is_too_long(&e) => run.out.errors.push("A message Resend received is too large to download".into()),
            Err(e) => {
                // What came after it waits for the next run, with it.
                run.failed("Couldn't download a message Resend received", &e);
                whole = false;
                break;
            }
        }
        delta.uid_seen = uid;
        if delta.new.len() >= BATCH_MESSAGES {
            let next = FolderDelta { uid_seen: delta.uid_seen, ..blank.clone() };
            run.out.folders.push(std::mem::replace(&mut delta, next));
            run.hand_over(SyncStep::new(SyncStage::Reading, "Inbox", done + 1, total));
        }
    }
    delta.complete = whole;
    run.out.folders.push(delta);
}

/// Resend keeps no flags or folders: a change made here is done once it is made here. A message
/// put in the account from elsewhere can't go to Resend, and stays where it is.
fn settle_changes(run: &mut Run) {
    let request = run.request;
    for op in &request.ops {
        match op {
            RemoteOp::Flags { folder, uid, read, flagged, answered } => {
                run.out.flagged.push(FlagsPushed { folder: folder.clone(), uid: *uid, read: *read, flagged: *flagged, answered: *answered })
            }
            RemoteOp::Move { message, from, uid, to_folder, .. } => {
                run.out.moved.push(Moved { message: *message, from: from.clone(), uid: *uid, to_folder: *to_folder, new_uid: 0, uid_validity: 0 })
            }
            RemoteOp::Delete { from, uid } => run.out.deleted.push(Deleted { from: from.clone(), uid: *uid }),
        }
    }
    if !request.places.is_empty() {
        run.out.unplaced.extend(request.places.iter().map(|place| place.message.id));
        run.out.errors.push("Resend only keeps the mail it received: what was put in the account stays on this computer".into());
    }
}

/// One message of Resend's list: its id, and when it came (seconds since 1970, and the
/// thousandths of that second).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Listed {
    pub(super) id: String,
    pub(super) secs: i64,
    pub(super) millis: u32,
}

/// What Resend received from the second the UID `seen` was made from on (everything, for 0), as
/// far as [`MAX_PAGES`] go: the whole of that second, so that its messages are numbered as before.
fn list_since(key: &str, seen: u32) -> Result<Vec<Listed>, NetError> {
    let from = BASE.saturating_add(i64::from(seen) / SLOTS);
    let mut out: Vec<Listed> = Vec::new();
    for _ in 0..MAX_PAGES {
        let path = match out.last() {
            Some(last) => format!("/emails/receiving?limit=100&after={}", http::encode(&last.id)),
            None => "/emails/receiving?limit=100".into(),
        };
        let (page, more) =
            read_page(&get(key, &path)?).ok_or_else(|| NetError::Proto(ProtoError::Rejected("Resend's list of mail made no sense".into())))?;
        let ended = page.is_empty() || page.iter().any(|listed| listed.secs < from);
        out.extend(page);
        if ended || !more {
            break;
        }
    }
    Ok(out)
}

/// One page of `GET /emails/receiving`: its messages (those without a usable id or date left
/// out) and whether more follow.
pub(super) fn read_page(body: &[u8]) -> Option<(Vec<Listed>, bool)> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let more = v.get("has_more").and_then(Value::as_bool).unwrap_or(false);
    let page = v
        .get("data")?
        .as_array()?
        .iter()
        .take(100)
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str).filter(|id| is_id(id))?;
            let (secs, millis) = instant(entry.get("created_at").and_then(Value::as_str)?)?;
            Some(Listed { id: id.to_string(), secs, millis })
        })
        .collect();
    Some((page, more))
}

/// The messages oldest first, each with its UID: the steps of [`SLOTS`] since [`BASE`] to the
/// second it came in, and which of that second's messages it is.
pub(super) fn numbered(mut listed: Vec<Listed>) -> Vec<(u32, Listed)> {
    listed.sort_by(|a, b| (a.secs, a.millis, &a.id).cmp(&(b.secs, b.millis, &b.id)));
    listed.dedup_by(|a, b| a.id == b.id);
    let mut out: Vec<(u32, Listed)> = Vec::with_capacity(listed.len());
    let mut in_second = 0;
    for (at, entry) in listed.into_iter().enumerate() {
        let same = at > 0 && out.last().is_some_and(|(_, last)| last.secs == entry.secs);
        in_second = if same { (in_second + 1).min(SLOTS - 1) } else { 0 };
        let step = entry.secs.saturating_sub(BASE).max(0).saturating_mul(SLOTS).saturating_add(in_second);
        // Never 0, which says nothing was seen.
        let uid = u32::try_from(step).unwrap_or(u32::MAX).max(1);
        out.push((uid, entry));
    }
    out
}

/// `2026-10-09T14:37:40.951Z` as seconds since 1970 and thousandths.
pub(super) fn instant(text: &str) -> Option<(i64, u32)> {
    if text.len() > 64 {
        return None;
    }
    let secs = mailcraft_time::iso_to_timestamp(text, 0)?;
    let fraction = text.split_once('.').map_or("", |(_, rest)| rest);
    let digits: String = fraction.chars().take_while(char::is_ascii_digit).take(3).collect();
    let millis = format!("{digits:0<3}").parse().unwrap_or(0);
    Some((secs, millis))
}

/// An id Resend hands out: letters, digits and dashes, short. Nothing else goes into an address.
pub(super) fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The bytes of the message Resend knows as `id`: where they are, then the bytes themselves, of
/// at most `largest`.
fn fetch(key: &str, id: &str, largest: u64) -> Result<Vec<u8>, NetError> {
    if !is_id(id) {
        return Err(NetError::Config("Resend named a message by an id that isn't one".into()));
    }
    let said: Value = serde_json::from_slice(&get(key, &format!("/emails/receiving/{id}"))?)
        .map_err(|_| NetError::Proto(ProtoError::Rejected("Resend's word on a message made no sense".into())))?;
    let address = said.pointer("/raw/download_url").and_then(Value::as_str).unwrap_or_default();
    let url = Url::parse_within(address, MAX_LONG_URL)?;
    raw_may_come_from(&url)?;
    let max_body = usize::try_from(largest).unwrap_or(usize::MAX).min(http::MAX_LARGE_BODY);
    // A signed address: nothing of the owner's goes with it, the key least of all.
    let reply = http::request(&url, &Request { method: "GET", headers: &[], body: None, patience: TIMEOUT, max_body })?;
    match reply.status {
        200 => Ok(reply.body),
        status => Err(refusal(status)),
    }
}

/// GET `path` of Resend's API with the key, and the answer's body when it is a yes.
fn get(key: &str, path: &str) -> Result<Vec<u8>, NetError> {
    // A key that isn't one is not sent anywhere; it is also what a header can't carry.
    if key.is_empty() || key.len() > MAX_KEY || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Proto(ProtoError::Auth("type the Resend API key as the account's password".into())));
    }
    let url = Url { host: RESEND_API_HOST.into(), port: 0, secure: true, path: path.into() };
    may_go_to(&url)?;
    let bearer = format!("Bearer {key}");
    let headers = [("Authorization", bearer.as_str()), ("Accept", "application/json")];
    let reply = http::request(&url, &Request { method: "GET", headers: &headers, body: None, patience: TIMEOUT, max_body: MAX_ANSWER })
        .map_err(|e| e.without(key))?;
    match reply.status {
        200 => Ok(reply.body),
        401 | 403 => {
            Err(NetError::Proto(ProtoError::Auth("Resend didn't take the API key (reading received mail needs a key with full access)".into())))
        }
        status => Err(refusal(status)),
    }
}

/// What a status other than a yes says.
fn refusal(status: u16) -> NetError {
    match status {
        429 | 500..=599 => NetError::Busy { text: "Resend is busy; try again later".into(), retry_after: None },
        _ => NetError::Proto(ProtoError::Rejected(format!("Resend answered with status {status}"))),
    }
}

/// Whether the API key may go to `url`: Resend's API host, over TLS, on its usual port, and no
/// other place.
pub(super) fn may_go_to(url: &Url) -> Result<(), NetError> {
    if url.secure && url.port == 0 && mailcraft_model::provider::is_resend_host(&url.host) {
        return Ok(());
    }
    Err(NetError::Config("a Resend API key is only shown to Resend's API".into()))
}

/// Whether a message's bytes may be fetched from `url`: Resend's own, over TLS.
pub(super) fn raw_may_come_from(url: &Url) -> Result<(), NetError> {
    let host = url.host.trim_end_matches('.').to_ascii_lowercase();
    if url.secure && url.port == 0 && (host == "resend.com" || host.ends_with(".resend.com")) {
        return Ok(());
    }
    Err(NetError::Config("Resend said a message is somewhere that isn't Resend's".into()))
}

#[cfg(test)]
mod tests;
