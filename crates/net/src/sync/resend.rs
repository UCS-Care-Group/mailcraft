// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mail Resend received for the owner's domain, read from Resend's API: Resend has no IMAP.
//! What is sent goes through Resend's SMTP server like any other account's ([`super::send_outbox`]).
//!
//! An account at Resend names [`RESEND_API_HOST`] as its incoming server and its API key as its
//! password ([`mailcraft_model::Account::is_resend`]). The key goes to that host alone, over TLS,
//! as `Authorization: Bearer` ([`may_go_to`]), and never in an address, an error or a log line.
//! Two requests are made of it, as Resend's API reference has them: `GET /emails/receiving` lists
//! what came, newest first, a hundred at a time, and `GET /emails/receiving/{id}` says where the
//! message's bytes are; those are then fetched from the signed address Resend gives (on a host
//! under `resend.com`, `resend.app` or Amazon's `cloudfront.net`, over TLS, as are the redirects followed from
//! it), with nothing of the owner's sent along. The calls are paced ([`PACE`]): Resend's limit is
//! the whole team's. Resend lists what came to every domain of the team: the account keeps what
//! was written to its own address's domain ([`is_for`]).
//!
//! The account has one folder there, its Inbox ([`INBOX`]). Resend knows no flags, moves or
//! folders: what the owner changes here stays here, and the run says the change is done so that
//! it isn't asked again. Each message is given a UID made from when it came ([`numbered`]), so
//! that a run brings only what is newer than what the folder has.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_model::provider::RESEND_API_HOST;
use mailcraft_model::sync::{Deleted, FlagsPushed, FolderDelta, FolderState, ListedFolder, Moved, RemoteOp, SyncRequest, SyncStage, SyncStep};
use mailcraft_model::{Body, FolderKind, Message};
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::Fetched;
use serde_json::Value;

use super::{Run, state_for, too_large};
use crate::http::{self, MAX_LONG_URL, Request, Url};
use crate::{NetError, transport};

/// The one folder an account at Resend has on the server, as its path is kept.
const INBOX: &str = "INBOX";
/// The `UIDVALIDITY` the folder is kept with: the UIDs only ever mean what [`numbered`] says.
const UID_VALIDITY: u32 = 1;
/// UIDs count from this instant (2024-01-01T00:00:00Z)...
const BASE: i64 = 1_704_067_200;
/// ...in this many steps a second: messages that came in the same second are told apart by when
/// in it they came. shortcut: the UIDs last until 2041, and a ninth message in one second would
/// share the eighth's UID, so it isn't brought and the run says so ([`bring`]); give the folder a
/// new `UIDVALIDITY` and a later base, or more steps, when that is too little.
const SLOTS: i64 = 8;
/// How long a second has to settle before its messages are numbered: Resend takes mail in on its
/// own time, and one that turned up in the list after another of its second, but earlier in it,
/// would take that one's UID. shortcut: mail shows a minute late, and one that takes Resend longer
/// than that to list, or a clock here that is far ahead, can still do so.
const SETTLE: i64 = 60;
/// Most messages listed in a first run (100 a page): the newest.
const FIRST_PAGES: usize = 50;
/// Most listed in a later one, to reach where the folder was read to; what came before that, when
/// more came since, isn't brought, and the run says so.
const MAX_PAGES: usize = 200;
/// How long Resend's API has to be reached and to answer one request.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The largest list or message description read.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// The longest API key shown to Resend.
const MAX_KEY: usize = 1024;
/// The wait before each call to Resend's API. Resend lets a team make 10 calls a second, all of
/// its keys together: at 4 a second, the owner's own programs that send through Resend keep the rest.
const PACE: Duration = Duration::from_millis(250);
/// The longest wait for Resend after it said "not now" (429), before asking once more.
const MAX_RETRY_AFTER: u64 = 10;
/// Most redirects followed from the address of a message's bytes.
const MAX_REDIRECTS: usize = 3;

/// Check that Resend takes the API key and lets it read received mail: one list of one message.
pub(crate) fn check(key: &str) -> Result<(), NetError> {
    get(key, "/emails/receiving?limit=1", &AtomicBool::new(false)).map(|_| ())
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
    let (key, cancel) = (request.secret(), run.cancel);
    let (mut listed, reached) = match list_since(key, inbox(request).1, cancel) {
        Ok(listed) => listed,
        Err(e) => {
            run.failed("Couldn't check for new mail", &e);
            return;
        }
    };
    if !reached {
        run.out.errors.push(format!(
            "More mail came to Resend since the last Send/Receive than MailCraft lists in one ({} messages): the oldest of it wasn't brought; read it on Resend",
            MAX_PAGES * 100
        ));
    }
    let domain = request.account.email.rsplit_once('@').map_or("", |(_, domain)| domain);
    listed.retain(|listed| is_for(&listed.said, domain));
    let largest = run.limits.largest;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    bring(run, listed, now, &mut |id| fetch(key, id, largest, cancel));
}

/// The Inbox as the mailbox knows it, and the highest UID it has of it (0 for none, or for UIDs
/// made some other way).
fn inbox(request: &SyncRequest) -> (Option<&FolderState>, u32) {
    let state = state_for(request, INBOX, FolderKind::Inbox);
    (state, state.filter(|s| s.uid_validity == UID_VALIDITY).map_or(0, |s| s.uid_seen))
}

/// What a message's bytes came to ([`fetch`]).
pub(super) enum Got {
    Whole(Vec<u8>),
    /// Larger than is downloaded.
    TooLarge,
}

/// Bring down what of `listed` the Inbox doesn't have yet, oldest first, through `fetch`, as far
/// as the run's limits go; the seconds of the last [`SETTLE`] before `now` wait for the next run.
/// The folder's count (`uid_seen`) only moves past a message once it is here, whole or as its
/// header: one that couldn't be had for now (Resend busy, the connection lost) stops the run
/// there, and it and what came after it are asked for by the next run; one Resend can't give
/// (it says nothing of where its bytes are, or refuses them) is kept as its header and a note,
/// so that it doesn't hold back what comes after it.
pub(super) fn bring(run: &mut Run, listed: Vec<Listed>, now: i64, fetch: &mut dyn FnMut(&str) -> Result<Got, NetError>) {
    let request = run.request;
    let (state, seen) = inbox(request);
    let known: std::collections::HashSet<u32> = state.map(|s| s.known_uids.iter().copied().collect()).unwrap_or_default();
    let settled: Vec<Listed> = listed.into_iter().filter(|l| l.secs <= now.saturating_sub(SETTLE)).collect();
    let mut new: Vec<(u32, Listed)> = numbered(settled).into_iter().filter(|(uid, _)| *uid > seen && !known.contains(uid)).collect();
    // A UID two messages would share ([`SLOTS`]) is the first's.
    let before = new.len();
    new.dedup_by_key(|(uid, _)| *uid);
    if new.len() < before {
        run.out.errors.push(format!(
            "Resend received more than {SLOTS} messages in one second: {} of them weren't brought here; read them on Resend",
            before - new.len()
        ));
    }
    // The newest so many, when the folder keeps no more.
    let most = usize::try_from(request.max_per_folder).unwrap_or(usize::MAX);
    if most > 0 && new.len() > most {
        new.drain(..new.len() - most);
    }
    settle_again(run, state);
    let total = new.len();
    let blank = FolderDelta {
        id: state.map(|s| s.id),
        remote: INBOX.into(),
        kind: FolderKind::Inbox,
        uid_validity: UID_VALIDITY,
        uid_seen: seen,
        ..Default::default()
    };
    let limits = &run.limits;
    let (most_bytes, most_messages, batch_messages, batch_bytes, batch_wait) =
        (limits.bytes, limits.messages, limits.batch_messages, limits.batch_bytes, limits.batch_wait);
    let mut delta = blank.clone();
    let (mut whole, mut run_bytes, mut batch, mut batch_started) = (true, 0usize, 0usize, Instant::now());
    for (done, (uid, listed)) in new.into_iter().enumerate() {
        // What doesn't fit in this run comes with the next.
        if run.cancelled() || done >= most_messages || run_bytes >= most_bytes {
            whole = false;
            break;
        }
        run.asked = run.asked.saturating_add(2);
        let mut message = match fetch(&listed.id) {
            Ok(Got::Whole(bytes)) => {
                (run_bytes, batch) = (run_bytes.saturating_add(bytes.len()), batch.saturating_add(bytes.len()));
                mailcraft_mime::parse_fetched(&bytes)
            }
            Ok(Got::TooLarge) => header_only(&listed.said, None, run.limits.largest),
            Err(e) if is_for_good(&e) => {
                run.out.errors.push(format!("A message Resend received couldn't be downloaded, and is kept as its header: {e}"));
                header_only(&listed.said, Some(&e.to_string()), run.limits.largest)
            }
            Err(e) => {
                run.failed("Couldn't download a message Resend received", &e);
                whole = false;
                break;
            }
        };
        message.uid = uid;
        message.received = listed.secs;
        delta.new.push(message);
        delta.uid_seen = uid;
        if delta.new.len() >= batch_messages || batch >= batch_bytes || batch_started.elapsed() >= batch_wait {
            let next = FolderDelta { uid_seen: delta.uid_seen, ..blank.clone() };
            run.out.folders.push(std::mem::replace(&mut delta, next));
            run.hand_over(SyncStep::new(SyncStage::Reading, "Inbox", done + 1, total));
            (batch, batch_started) = (0, Instant::now());
        }
    }
    delta.complete = whole;
    run.out.folders.push(delta);
}

/// Messages the mailbox wants downloaded again ([`FolderState::again`]) are settled as they are:
/// Resend can't be asked for one by its UID here.
fn settle_again(run: &mut Run, state: Option<&FolderState>) {
    let Some(again) = state.map(|s| &s.again).filter(|again| !again.is_empty()) else { return };
    let mut done =
        FolderDelta { id: state.map(|s| s.id), remote: INBOX.into(), kind: FolderKind::Inbox, uid_validity: UID_VALIDITY, ..Default::default() };
    done.refetch_done.clone_from(again);
    run.out.folders.push(done);
    run.out.errors.push("Resend can't send a message it received again: it is kept as it came".into());
}

/// Whether Resend's answer for a message will be the same however often it is asked: an address
/// that isn't one, or a refusal. Not "not now", a lost connection or a refused key.
fn is_for_good(error: &NetError) -> bool {
    matches!(error, NetError::Config(_) | NetError::Proto(ProtoError::Rejected(_)))
}

/// A message whose bytes aren't here, as the IMAP side keeps one ([`too_large`]): who sent it,
/// to whom, when and about what, from what Resend's list said of it, and a note in place of its
/// text: that it is larger than `largest` (`failed` none; its size is what its attachments come
/// to, and more than `largest` in any case), or why it couldn't be downloaded.
fn header_only(said: &Value, failed: Option<&str>, largest: u64) -> Message {
    // What Resend says goes into a header: one line, no NUL, not too long.
    let line = |text: &str| text.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).take(900).collect::<String>();
    let text = |name: &str| said.get(name).and_then(Value::as_str).map(line).unwrap_or_default();
    let list = |name: &str| {
        let names: Vec<String> =
            said.get(name).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).take(100).map(line).collect();
        line(&names.join(", "))
    };
    let mut head = String::new();
    for (name, value) in
        [("From", text("from")), ("To", list("to")), ("Cc", list("cc")), ("Subject", text("subject")), ("Message-ID", text("message_id"))]
    {
        if !value.is_empty() {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str("\r\n");
    let mut message = match failed {
        None => {
            let attached: u64 = said
                .get("attachments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(1_000)
                .filter_map(|a| a.get("size").and_then(Value::as_u64))
                .fold(0, u64::saturating_add);
            let header = Fetched { body: Some(head.into_bytes()), ..Default::default() };
            too_large(header, attached.max(largest.saturating_add(1)), largest)
        }
        Some(why) => {
            let mut message = mailcraft_mime::parse_fetched(head.as_bytes());
            let why: String = why.chars().take(300).collect();
            message.body = Body {
                text: format!(
                    "MailCraft couldn't download this message from Resend ({why}).\n\nWhat is here comes from Resend's list alone: who sent it, when, and what it is about. Read the message itself on Resend.\n"
                ),
                html: None,
            };
            message
        }
    };
    if let Some((secs, _)) = said.get("created_at").and_then(Value::as_str).and_then(instant) {
        message.date = secs;
    }
    message
}

/// Resend keeps no flags or folders: a change made here is done once it is made here. Nothing is
/// put into the account from elsewhere ([`mailcraft_model::Mailbox::puts_on_server`]); should a
/// mailbox from before ask for it, the message goes back where it came from.
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
        run.out.errors.push("Resend only keeps the mail it received: what was put in the account goes back where it came from".into());
    }
}

/// One message of Resend's list: its id, when it came (seconds since 1970, and the thousandths of
/// that second), and what the list said of it (for its header, when its bytes can't be had).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Listed {
    pub(super) id: String,
    pub(super) secs: i64,
    pub(super) millis: u32,
    pub(super) said: Value,
}

/// What Resend received from the second the UID `seen` was made from on (the newest, for 0), and
/// whether the list reached that second: the whole of it, so that its messages are numbered as
/// before.
fn list_since(key: &str, seen: u32, cancel: &AtomicBool) -> Result<(Vec<Listed>, bool), NetError> {
    let from = BASE.saturating_add(i64::from(seen) / SLOTS);
    let pages = if seen == 0 { FIRST_PAGES } else { MAX_PAGES };
    let mut out: Vec<Listed> = Vec::new();
    for _ in 0..pages {
        let path = match out.last() {
            Some(last) => format!("/emails/receiving?limit=100&after={}", http::encode(&last.id)),
            None => "/emails/receiving?limit=100".into(),
        };
        let (page, more) = read_page(&get(key, &path, cancel)?)
            .ok_or_else(|| NetError::Proto(ProtoError::Rejected("Resend's list of mail made no sense".into())))?;
        let ended = page.is_empty() || page.iter().any(|listed| listed.secs < from);
        out.extend(page);
        if ended || !more {
            return Ok((out, true));
        }
    }
    // A first run takes the newest so many by design.
    Ok((out, seen == 0))
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
            Some(Listed { id: id.to_string(), secs, millis, said: entry.clone() })
        })
        .collect();
    Some((page, more))
}

/// Whether a message of Resend's list was written to an address at `domain` (To, Cc or Bcc as the
/// list gives them, `Name <a@b>` or bare).
pub(super) fn is_for(said: &Value, domain: &str) -> bool {
    let domain = domain.trim().trim_end_matches('.');
    !domain.is_empty()
        && ["to", "cc", "bcc"].iter().filter_map(|field| said.get(field).and_then(Value::as_array)).flatten().take(300).filter_map(Value::as_str).any(
            |text| {
                let address = text.rsplit_once('<').map_or(text, |(_, rest)| rest.split('>').next().unwrap_or(rest));
                address.trim().rsplit_once('@').is_some_and(|(_, at)| at.trim_end_matches('.').eq_ignore_ascii_case(domain))
            },
        )
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
/// at most `largest`, following a redirect that stays with Resend.
fn fetch(key: &str, id: &str, largest: u64, cancel: &AtomicBool) -> Result<Got, NetError> {
    if !is_id(id) {
        return Err(NetError::Config("Resend named a message by an id that isn't one".into()));
    }
    let said: Value = serde_json::from_slice(&get(key, &format!("/emails/receiving/{id}"), cancel)?)
        .map_err(|_| NetError::Proto(ProtoError::Rejected("Resend's word on a message made no sense".into())))?;
    let mut address = said.pointer("/raw/download_url").and_then(Value::as_str).unwrap_or_default().to_string();
    let max_body = usize::try_from(largest).unwrap_or(usize::MAX).min(http::MAX_LARGE_BODY);
    for _ in 0..=MAX_REDIRECTS {
        let url = Url::parse_within(&address, MAX_LONG_URL)?;
        raw_may_come_from(&url)?;
        stop_if(cancel)?;
        // A signed address: nothing of the owner's goes with it, the key least of all.
        let request = Request { method: "GET", headers: &[], body: None, patience: TIMEOUT, max_body };
        let reply = match crate::kept::request(&url, &request, true) {
            Ok(reply) => reply,
            Err(e) if http::is_too_long(&e) => return Ok(Got::TooLarge),
            Err(e) => return Err(e),
        };
        match (reply.status, reply.location) {
            (200, _) => return Ok(Got::Whole(reply.body)),
            (301 | 302 | 303 | 307 | 308, Some(next)) => address = next,
            (status, _) => return Err(refusal(status)),
        }
    }
    Err(NetError::Proto(ProtoError::Rejected("Resend sent a message's bytes on too many redirects".into())))
}

/// GET `path` of Resend's API with the key, and the answer's body when it is a yes. After a "not
/// now" (429), asked once more after the wait Resend asked for (at most [`MAX_RETRY_AFTER`]).
fn get(key: &str, path: &str, cancel: &AtomicBool) -> Result<Vec<u8>, NetError> {
    // A key that isn't one is not sent anywhere; it is also what a header can't carry.
    if key.is_empty() || key.len() > MAX_KEY || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Proto(ProtoError::Auth("type the Resend API key as the account's password".into())));
    }
    let url = Url { host: RESEND_API_HOST.into(), port: 0, secure: true, path: path.into() };
    may_go_to(&url)?;
    let bearer = format!("Bearer {key}");
    let headers = [("Authorization", bearer.as_str()), ("Accept", "application/json")];
    let request = Request { method: "GET", headers: &headers, body: None, patience: TIMEOUT, max_body: MAX_ANSWER };
    let mut wait = PACE;
    for asked in 0..2 {
        pause(wait, cancel)?;
        // (A read: asked again on a new connection when one that waited turns out closed.)
        let reply = crate::kept::request(&url, &request, true).map_err(|e| e.without(key))?;
        match reply.status {
            200 => return Ok(reply.body),
            401 | 403 => {
                return Err(NetError::Proto(ProtoError::Auth(
                    "Resend didn't take the API key (reading received mail needs a key with full access)".into(),
                )));
            }
            429 if asked == 0 => {
                let said = reply.retry_after.as_deref().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(1);
                wait = Duration::from_secs(said.clamp(1, MAX_RETRY_AFTER));
            }
            status => return Err(refusal(status)),
        }
    }
    Err(refusal(429))
}

/// Wait `wait`, a little at a time, and stop when the run is called off.
fn pause(wait: Duration, cancel: &AtomicBool) -> Result<(), NetError> {
    let until = Instant::now() + wait;
    loop {
        stop_if(cancel)?;
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(PACE));
    }
}

/// The error of a run called off ([`transport::STOPPED`]), when it was.
fn stop_if(cancel: &AtomicBool) -> Result<(), NetError> {
    if cancel.load(Ordering::Relaxed) { Err(NetError::Proto(ProtoError::Io(transport::STOPPED.into()))) } else { Ok(()) }
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

/// Whether a message's bytes may be fetched from `url`: Resend's own (`cdn.resend.app` as seen), or the CloudFront address
/// Resend's API reference says it signs, over TLS.
pub(super) fn raw_may_come_from(url: &Url) -> Result<(), NetError> {
    let host = url.host.trim_end_matches('.').to_ascii_lowercase();
    let under = |domain: &str| host == domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.'));
    if url.secure && url.port == 0 && (under("resend.com") || under("resend.app") || under("cloudfront.net")) {
        return Ok(());
    }
    // The host alone: the signature is in the rest of the address.
    Err(NetError::Config(format!("Resend said a message is at {host}, which isn't Resend's")))
}

#[cfg(test)]
mod tests;
