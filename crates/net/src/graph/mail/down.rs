// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's mail down through Graph's message delta (R3.2; R3 spec S9–S11, S14, S15).
//!
//! After the folders, each folder's messages are read by `GET
//! /me/mailFolders/{id}/messages/delta`, newest first, a hundred to a page, asking for what the
//! message list needs and no more. A folder goes on from its link (the next page of a round under
//! way, or its delta link once it was read to the end); one without starts at the top.
//!
//! - **Order.** First the newest page of every folder (the Inbox, the special folders in
//!   Outlook's order, then the rest), so that every folder shows mail within minutes; then each
//!   folder to its end, in the same order. A folder that has been read to its end once goes to
//!   the end of what changed since at once: as a rule that is a page.
//! - **A page at a time.** Every page is handed on as it comes, with the link that goes on from
//!   it, to be taken in and kept together. Cancel is looked at before every page.
//! - **A link the server no longer takes** (`410 Gone`, or `syncStateNotFound`) starts the folder
//!   afresh, and says so with its first page (S11). So does a link from the mailbox that isn't
//!   one of Graph's own: it is never followed.
//! - **Stopping.** The server busy, the sign-in refused or the network gone ends the run where it
//!   is; another folder's trouble (gone meanwhile, refused) leaves that folder for the next run.
//!   Only a run in which every folder got to its end says so ([`GraphMailResult::whole`]): the end
//!   of a round begun in this run, as one begun in an earlier run sees the mailbox as it was then.
//! - **No sizes.** A delta isn't asked for the messages' sizes: the size is an extended property
//!   (`PidTagMessageSize`) that only an `$expand` with a `$filter` on its id brings, and Exchange
//!   Online refuses that on a delta (`400 BadRequest`, "Parsing OData Select and Expand failed"),
//!   though Graph documents `$expand` for it. A message comes with size 0, "not said", and is
//!   fetched whole in the background like a small one ("Download messages in full" can't hold it
//!   back; the fetch has its own cap).
//! - **Refusals are said.** A folder the server won't list (other than one gone meanwhile) is
//!   counted, with Graph's error code in the log, and the run ends saying how many there were.
//! - **A change of read state alone** comes as the message's id and `isRead`, nothing else: so
//!   Exchange Online says a message marked read or unread elsewhere (Graph's delta docs: read
//!   state changes are events of the folder's synchronisation, not changes of the message). It is
//!   taken as that, never as a message (#148).
//! - **What comes back is hostile JSON.** Every id, text and list is capped; an entry that can't
//!   be read is passed over. Nothing of a message or of a link goes into a log line or an error.
//! - **Folders that haven't changed are passed over** (#276). A folder of the owner's own (not
//!   the Inbox nor another special folder, which are always read) whose delta link ended a round
//!   and whose message and unread counts in this run's listing are the ones kept with that link
//!   is not asked for, and is counted in the run's timings as skipped. The counts are a hint, not
//!   a proof: one message in and one out, a message read and another marked unread, or a flag or
//!   a category changed leave them as they were. So the engine asks for a **full sweep**
//!   ([`GraphMailRequest::full`]), with nothing passed over, on the first run after the mailbox
//!   is opened, once an hour after that, and while a message the server said left its folder
//!   waits for a whole run: what a skip missed comes then, from the same delta link, so nothing
//!   is lost, only late. And when a folder read in this run says a message left it that turned
//!   up in no folder read, the folders passed over are read after all: it may have moved into
//!   one of them, and a run that is whole removes what is still gone ([`GraphMailResult::whole`]).
//! - **An empty page changes nothing** (#276): a page from a delta link that brings nothing, and
//!   whose counts are the ones kept, is not handed on (the piece has its progress and timings
//!   only): the link kept goes on from the same place, and the mailbox isn't touched.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use mailcraft_model::sync::{
    GraphFolder, GraphMailRequest, GraphMailResult, GraphMessage, GraphProgress, MessagePage, ReadState, ServerMessage, SyncPhase, Timings,
};
use mailcraft_model::{Address, FlagStatus, FolderCounts, FolderKind, Importance};
use serde_json::Value;

use super::super::pace::Kind;
use super::super::{ASK_TIMEOUT, Graph, MAX_PAGE, code_of, logged_code, nonsense, text};
use super::{folders as folders_of, id_of};
use crate::NetError;
use crate::http::{self, Url};

/// What is asked for of each message: what the message list needs (R3 spec S9).
pub(super) const FIELDS: &str = "receivedDateTime,sentDateTime,subject,from,toRecipients,ccRecipients,bodyPreview,isRead,flag,categories,importance,parentFolderId,internetMessageId,hasAttachments,isDraft,changeKey";
/// A message's size on the server (`PidTagMessageSize`), which Graph gives only as an extended
/// property: what "Download messages in full" is held against. Asked for of a message by its id,
/// never of a delta, which refuses it.
pub(super) const SIZE: &str = "singleValueExtendedProperties($filter=id eq 'Integer 0x0E08')";
/// Messages to a page.
const PAGE_SIZE: &str = "odata.maxpagesize=100";
/// Most pages of one folder in one run (a million messages): a server that never ends a round
/// doesn't keep a run going for ever.
const MAX_FOLDER_PAGES: u32 = 10_000;
const MAX_TEXT: usize = 998;
const MAX_PREVIEW: usize = 255;
const MAX_PEOPLE: usize = 500;
const MAX_CATEGORIES: usize = 25;
/// Most message ids a run keeps to tell whether what left a folder turned up in another: past
/// that, every folder passed over is read after all.
const MAX_TRACKED: usize = 50_000;
/// The special folders first, in Outlook's order; then the rest as listed.
const ORDER: [FolderKind; 7] =
    [FolderKind::Inbox, FolderKind::Drafts, FolderKind::Sent, FolderKind::Deleted, FolderKind::Archive, FolderKind::Junk, FolderKind::Outbox];

/// One folder as the run reads it.
struct Reading {
    folder: GraphFolder,
    /// Where it goes on from; `None`: its top.
    link: Option<Url>,
    caught_up: bool,
    /// Its next page starts it afresh (its old link was no longer taken).
    afresh: bool,
    /// It got to its end in this run, or it is left for the next one.
    done: bool,
    left: bool,
    read: u32,
    /// Of `read`, changes of read state alone.
    read_alone: u32,
    pages: u32,
    /// Started afresh in this run already: a second time leaves it for the next one.
    restarted: bool,
    /// The round under way began in this run. One that began in an earlier run sees the mailbox
    /// as it was then: at its end, one more round brings what changed since.
    round_here: bool,
    /// The counts kept with its delta link, and those of this run's listing (`None` for the
    /// Inbox pass, which lists nothing).
    kept: Option<FolderCounts>,
    listed: Option<FolderCounts>,
    /// Passed over: its counts are the ones kept with its delta link (#276).
    skipped: bool,
}

/// The time and requests since the last piece was handed on, for the run's timings (#276).
struct Lap {
    from: Instant,
    requests: u32,
    /// Whose time it is: new mail's, or the Inbox pass's.
    phase: SyncPhase,
}

impl Lap {
    fn start(graph: &Graph<'_>, phase: SyncPhase) -> Lap {
        Lap { from: Instant::now(), requests: graph.requests_asked(), phase }
    }

    /// What has gone since the last lap, as the phase's time; the next lap begins now.
    fn take(&mut self, graph: &Graph<'_>) -> Timings {
        let mut timings = Timings::default();
        let asked = graph.requests_asked();
        timings.add_time(self.phase, self.from.elapsed(), asked.saturating_sub(self.requests));
        *self = Lap { from: Instant::now(), requests: asked, phase: self.phase };
        timings
    }
}

fn phase_is_inbox(phase: SyncPhase) -> bool {
    phase == SyncPhase::Inbox
}

/// What the run has met of the server, folder by folder.
#[derive(Default)]
struct Run {
    /// Folders the server refused to list (not gone meanwhile), and the code of the first refusal.
    refused: u32,
    code: String,
    /// While folders are passed over: the ids of messages that left a folder read, and of those
    /// a folder read has; past [`MAX_TRACKED`] of them, `overflow`.
    tracking: bool,
    left: HashSet<String>,
    seen: HashSet<String>,
    overflow: bool,
}

impl Run {
    fn note(&mut self, page: &MessagePage) {
        if !self.tracking || self.overflow {
            return;
        }
        self.left.extend(page.removed.iter().cloned());
        self.seen.extend(page.messages.iter().map(|m| m.server.id.clone()));
        if self.left.len().saturating_add(self.seen.len()) > MAX_TRACKED {
            self.overflow = true;
            (self.left, self.seen) = Default::default();
        }
    }

    /// A message left a folder read and turned up in none read: it may be in one passed over.
    fn unexplained(&self) -> bool {
        self.overflow || self.left.iter().any(|id| !self.seen.contains(id))
    }
}

/// Whether `one` may be passed over (#276): a folder of the owner's own whose delta link ended a
/// round and whose counts in this run's listing are those kept with the link. When in doubt, no.
fn unchanged(one: &Reading) -> bool {
    one.folder.kind == FolderKind::Custom && one.link.is_some() && one.caught_up && !one.afresh && one.kept.is_some() && one.kept == one.listed
}

/// Why the run stopped.
enum Stop {
    Cancelled,
    Error(NetError),
}

/// Read every folder of `listed`, handing each page on through `emit` as a piece of the run; the
/// last piece is the answer for the end of the run.
pub(super) fn messages(
    graph: &Graph<'_>,
    request: &GraphMailRequest,
    listed: &[GraphFolder],
    phase: SyncPhase,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(GraphMailResult),
) -> GraphMailResult {
    let mut out = GraphMailResult { account: request.account, email: request.email.clone(), ..Default::default() };
    let links: HashMap<&str, (&str, bool, Option<FolderCounts>)> =
        request.links.iter().map(|l| (l.folder.as_str(), (l.link.as_str(), l.caught_up, l.counts))).collect();
    // The Inbox pass lists nothing: its folder's counts are none of the server's.
    let listing = !phase_is_inbox(phase);
    let mut folders: Vec<GraphFolder> = listed.to_vec();
    folders.sort_by_key(|f| ORDER.iter().position(|kind| *kind == f.kind).unwrap_or(ORDER.len()));
    let mut reading: Vec<Reading> = folders
        .into_iter()
        .map(|folder| {
            let (link, caught_up, afresh, kept) = match links.get(folder.id.as_str()) {
                None => (None, false, false, None),
                // A link from the mailbox file is followed only to Graph itself.
                Some((link, caught_up, kept)) => match Url::parse_within(link, http::MAX_LONG_URL).ok().filter(|url| graph.base.same_server(url)) {
                    Some(url) => (Some(url), *caught_up, false, kept.filter(|_| *caught_up)),
                    None => (None, false, true, None),
                },
            };
            let round_here = link.is_none() || caught_up;
            // Counts the server didn't give are unknown, not 0: such a folder is never passed over.
            let listed = (listing && folder.counted).then_some(FolderCounts { total: folder.total, unread: folder.unread });
            let mut one = Reading {
                folder,
                link,
                caught_up,
                afresh,
                done: false,
                left: false,
                read: 0,
                read_alone: 0,
                pages: 0,
                restarted: false,
                round_here,
                kept,
                listed,
                skipped: false,
            };
            if listing && !request.full && unchanged(&one) {
                (one.skipped, one.done) = (true, true);
            }
            one
        })
        .collect();
    let count = u32::try_from(reading.len()).unwrap_or(u32::MAX);
    let mut lap = Lap::start(graph, phase);
    // First the newest page of each (all of what changed, for one read to its end before), then
    // each to its end.
    let mut stopped = None;
    let mut run = Run { tracking: reading.iter().any(|one| one.skipped), ..Default::default() };
    'passes: for (pass, first) in [(0, true), (1, false), (2, false)] {
        // After the rest: what left a folder read and turned up in none may be in a folder passed
        // over, so those are read after all (each from its delta link: as a rule, a page).
        if pass == 2 {
            if !run.tracking || !run.unexplained() {
                break;
            }
            log::info!("graph mail: mail left a folder and turned up in none read; the folders passed over are read too");
            for one in reading.iter_mut().filter(|one| one.skipped) {
                (one.skipped, one.done) = (false, false);
            }
            run.tracking = false;
        }
        for (n, one) in reading.iter_mut().enumerate() {
            while !one.done && !one.left {
                if cancel.load(Ordering::Relaxed) {
                    stopped = Some(Stop::Cancelled);
                    break 'passes;
                }
                let started = one.pages;
                if let Err(e) = page(graph, one, &mut run, u32::try_from(n + 1).unwrap_or(u32::MAX), count, &mut out, &mut lap, emit) {
                    stopped = Some(Stop::Error(e));
                    break 'passes;
                }
                if first && !one.caught_up && one.pages > started {
                    break;
                }
            }
        }
    }
    match stopped {
        Some(Stop::Cancelled) => {
            log::info!("graph mail: stopped when asked, after the page in hand");
            out.stopped = true;
        }
        Some(Stop::Error(e)) => {
            if let NetError::Busy { retry_after, .. } = &e {
                out.busy_for = Some(retry_after.unwrap_or(0));
            }
            log::info!("graph mail: the run stopped: {e}");
            out.errors.push(format!("Couldn't bring the mail down from Microsoft: {e}"));
        }
        None => {}
    }
    if run.refused > 0 {
        let folders = if run.refused == 1 { "1 folder".to_string() } else { format!("{} folders", run.refused) };
        out.errors.push(format!("Microsoft Graph refused to list the mail in {folders}{}; it is asked for again in the next run.", run.code));
    }
    out.timings = lap.take(graph);
    let skipped = reading.iter().filter(|one| one.skipped).count();
    out.timings.skipped = u32::try_from(skipped).unwrap_or(u32::MAX);
    out.inbox_pass = phase_is_inbox(phase);
    // (A folder passed over counts as done: nothing that left a folder read can be in it.)
    out.whole = reading.iter().all(|one| one.done);
    let pages: u32 = reading.iter().map(|one| one.pages).sum();
    let messages: u32 = reading.iter().map(|one| one.read).sum();
    let read_alone: u32 = reading.iter().map(|one| one.read_alone).sum();
    log::info!(
        "graph mail: {pages} pages, {messages} messages and changes ({read_alone} of read state alone), {} folders of {count} done, {skipped} passed over unchanged",
        reading.iter().filter(|one| one.done).count()
    );
    out
}

/// Read one page of `one` and hand it on. An error that ends the run comes back; a folder's own
/// trouble leaves it for the next run.
fn page(
    graph: &Graph<'_>,
    one: &mut Reading,
    run: &mut Run,
    number: u32,
    folders: u32,
    out: &mut GraphMailResult,
    lap: &mut Lap,
    emit: &mut dyn FnMut(GraphMailResult),
) -> Result<(), NetError> {
    if one.pages >= MAX_FOLDER_PAGES {
        one.left = true;
        return Ok(());
    }
    let url = match &one.link {
        Some(url) => url.clone(),
        None => {
            // (Not the sizes: a delta refuses their expansion, see above.)
            let query = http::form(&[("$select", FIELDS), ("$orderby", "receivedDateTime desc")]);
            graph.url(&format!("{}/{}/messages/delta?{query}", folders_of(graph), http::encode(&one.folder.id)))
        }
    };
    let reply = graph.call(Kind::Read, &url, "GET", &[("Prefer", PAGE_SIZE)], None, ASK_TIMEOUT, MAX_PAGE)?;
    one.pages += 1;
    match reply.status {
        200 => {}
        // The link is no longer taken: the folder starts afresh, once a run.
        400 | 410 if expired(&reply.body) || reply.status == 410 => {
            if one.restarted {
                one.left = true;
            } else {
                log::info!("graph mail: a folder's link was no longer taken; it is listed afresh");
                one.restarted = true;
                one.link = None;
                one.caught_up = false;
                one.afresh = true;
                one.round_here = true;
            }
            return Ok(());
        }
        401 | 403 | 429 | 503 | 504 => return Err(graph.refusal(super::super::Doing::Mail, &reply, "")),
        // Gone meanwhile, or not to be read: the next run's listing says.
        status => {
            let code = code_of(&reply.body, &[graph.token, &one.folder.id]);
            let logged = logged_code(&reply.body, &[graph.token, &one.folder.id]);
            log::info!("graph mail: a folder's mail couldn't be read (status {status}{logged}); it waits for the next run");
            // A folder gone meanwhile is for the next listing to say; anything else is told.
            if status != 404 {
                if run.refused == 0 {
                    run.code = if code.is_empty() { format!(" (status {status})") } else { code };
                }
                run.refused = run.refused.saturating_add(1);
            }
            one.left = true;
            return Ok(());
        }
    }
    let json = serde_json::from_slice::<Value>(&reply.body).map_err(|_| nonsense("Microsoft Graph's answer isn't the JSON it should be"))?;
    let entries = json.get("value").and_then(Value::as_array).ok_or_else(|| nonsense("Microsoft Graph's answer has no list in it"))?;
    let next = link_of(graph, &json, "@odata.nextLink");
    let delta = link_of(graph, &json, "@odata.deltaLink");
    let ((given, link), caught_up) = match (next, delta) {
        (Some(next), None) => (next, false),
        (None, Some(delta)) => (delta, true),
        // Neither, both, or one that can't be followed: the folder waits for the next run.
        _ => {
            log::info!("graph mail: a page named no way on that can be followed; the folder waits for the next run");
            one.left = true;
            return Ok(());
        }
    };
    let mut taken = MessagePage { folder: one.folder.id.clone(), afresh: std::mem::take(&mut one.afresh), caught_up, ..Default::default() };
    for entry in entries.iter().take(1_000) {
        match entry_of(entry, &one.folder.id) {
            Some(Entry::Message(message)) => taken.messages.push(*message),
            Some(Entry::Removed(id)) => taken.removed.push(id),
            Some(Entry::Read(id, read)) => taken.read.push(ReadState { id, read }),
            None => {}
        }
    }
    let read_alone = u32::try_from(taken.read.len()).unwrap_or(u32::MAX);
    one.read_alone = one.read_alone.saturating_add(read_alone);
    one.read = one.read.saturating_add(u32::try_from(taken.messages.len() + taken.removed.len()).unwrap_or(u32::MAX)).saturating_add(read_alone);
    taken.link = given;
    // The counts of this run's listing go with a link that ends a round begun after it; a round
    // begun in an earlier run may have missed what changed since then, and gets none.
    taken.counts = if caught_up && one.round_here { one.listed } else { None };
    run.note(&taken);
    // Nothing to take in, from a delta link to one, with the counts kept: the link kept goes on
    // from the same place, and the page isn't handed on.
    let empty = one.caught_up
        && caught_up
        && !taken.afresh
        && taken.messages.is_empty()
        && taken.removed.is_empty()
        && taken.read.is_empty()
        && taken.counts.is_some()
        && taken.counts == one.kept;
    one.link = Some(link);
    one.caught_up = caught_up;
    // At the end of a round begun in an earlier run, the next round (begun now) is what makes the
    // folder current.
    if caught_up {
        one.done = one.round_here;
        one.round_here = true;
    }
    let progress = GraphProgress { folder: one.folder.id.clone(), read: one.read, total: one.folder.total, number, folders };
    emit(GraphMailResult {
        account: out.account,
        email: out.email.clone(),
        pages: if empty { Vec::new() } else { vec![taken] },
        partial: true,
        progress: Some(progress),
        timings: lap.take(graph),
        inbox_pass: phase_is_inbox(lap.phase),
        ..Default::default()
    });
    Ok(())
}

/// Whether a refusal says the link is no longer known (`syncStateNotFound` and its kin).
fn expired(body: &[u8]) -> bool {
    let json = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let code = json.get("error").and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    code.contains("syncstate") || code.contains("resync")
}

/// A link of a page that may be followed (on Graph itself, where the token belongs), as it was
/// given and as it is followed.
fn link_of(graph: &Graph<'_>, json: &Value, name: &str) -> Option<(String, Url)> {
    let link = json.get(name)?.as_str()?;
    let url = Url::parse_within(link, http::MAX_LONG_URL).ok().filter(|url| graph.base.same_server(url))?;
    Some((link.to_string(), url))
}

/// The size among a message's extended properties (`Integer 0xe08`, however its id is written).
fn size_of(value: &Value) -> u64 {
    let all = value.get("singleValueExtendedProperties").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    all.iter()
        .take(16)
        .find(|p| text(p, "id").to_ascii_lowercase().replace("0x0e08", "0xe08").ends_with("0xe08"))
        .and_then(|p| text(p, "value").trim().parse::<u64>().ok())
        .unwrap_or(0)
}

enum Entry {
    Message(Box<GraphMessage>),
    Removed(String),
    /// Its read state alone.
    Read(String, bool),
}

fn capped(value: &Value, name: &str, most: usize) -> String {
    text(value, name).chars().filter(|c| !c.is_control() || *c == ' ').take(most).collect()
}

fn address_of(value: &Value) -> Option<Address> {
    let at = value.get("emailAddress")?;
    let email = capped(at, "address", 320);
    let name = capped(at, "name", 256);
    (!email.is_empty() || !name.is_empty()).then_some(Address { name, email })
}

fn people(value: &Value, name: &str) -> Vec<Address> {
    value.get(name).and_then(Value::as_array).map(|all| all.iter().take(MAX_PEOPLE).filter_map(address_of).collect()).unwrap_or_default()
}

/// A date of a message's follow-up flag (`{"dateTime", "timeZone"}`), when it has one.
fn flag_date(value: &Value, name: &str) -> Option<mailcraft_time::Timestamp> {
    super::super::when(value.get("flag")?.get(name).filter(|date| !date.is_null()))
}

/// A message as `GET /me/messages/{id}` gives it (asked for with [`FIELDS`] and [`SIZE`]), in
/// the folder it names.
pub(super) fn message_of(value: &Value) -> Option<GraphMessage> {
    let folder = id_of(text(value, "parentFolderId"))?;
    match entry_of(value, &folder)? {
        Entry::Message(message) => Some(*message),
        Entry::Removed(_) | Entry::Read(..) => None,
    }
}

/// A message's `isRead`: `true` or `false`, or the same as text (as Graph's own example of a
/// delta writes it).
fn is_read(value: &Value) -> Option<bool> {
    match value.get("isRead")? {
        Value::Bool(read) => Some(*read),
        Value::String(read) if read.eq_ignore_ascii_case("true") => Some(true),
        Value::String(read) if read.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// A message of a page, the id of one the server says left the folder, or a change of one's
/// read state alone.
fn entry_of(value: &Value, folder: &str) -> Option<Entry> {
    let id = id_of(text(value, "id"))?;
    if value.get("@removed").is_some() {
        return Some(Entry::Removed(id));
    }
    let read = is_read(value);
    let when = |name: &str| mailcraft_time::iso_to_timestamp(text(value, name), 0);
    // Without what a message is (when it arrived), it is a change of its read state alone.
    let Some(received) = when("receivedDateTime") else { return read.map(|read| Entry::Read(id, read)) };
    let flag = match value.get("flag").map(|f| text(f, "flagStatus")) {
        Some("flagged") => FlagStatus::Flagged,
        Some("complete") => FlagStatus::Complete,
        _ => FlagStatus::None,
    };
    let importance = match text(value, "importance") {
        "low" => Importance::Low,
        "high" => Importance::High,
        _ => Importance::Normal,
    };
    let categories = value
        .get("categories")
        .and_then(Value::as_array)
        .map(|all| all.iter().filter_map(Value::as_str).take(MAX_CATEGORIES).map(|c| c.chars().take(255).collect()).collect())
        .unwrap_or_default();
    let server = ServerMessage {
        id,
        folder: folder.to_string(),
        read: read.unwrap_or(false),
        // Graph says neither of the marks a JMAP server keeps as keywords.
        answered: false,
        forwarded: false,
        flag,
        categories,
        importance,
        gone: false,
        size: size_of(value),
        flag_start: flag_date(value, "startDateTime"),
        flag_due: flag_date(value, "dueDateTime"),
        flag_completed: flag_date(value, "completedDateTime"),
        // Visible ASCII, as Graph writes a change key; anything else is none.
        change_key: Some(text(value, "changeKey"))
            .filter(|key| key.len() <= 512 && key.bytes().all(|b| b.is_ascii_graphic()))
            .map(str::to_string)
            .unwrap_or_default(),
    };
    Some(Entry::Message(Box::new(GraphMessage {
        server,
        message_id: capped(value, "internetMessageId", MAX_TEXT),
        subject: capped(value, "subject", MAX_TEXT),
        from: value.get("from").and_then(address_of).unwrap_or_default(),
        to: people(value, "toRecipients"),
        cc: people(value, "ccRecipients"),
        sent: when("sentDateTime").unwrap_or(received),
        received,
        preview: capped(value, "bodyPreview", MAX_PREVIEW),
        has_attachments: value.get("hasAttachments").and_then(Value::as_bool).unwrap_or(false),
        draft: value.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Entry, entry_of};

    const ID: &str = "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A0000000001AAA=";

    #[test]
    fn a_change_of_read_state_alone_is_taken_as_that_and_never_as_a_message() {
        let read = |value| match entry_of(&value, "F") {
            Some(Entry::Read(id, read)) => Some((id == ID, read)),
            _ => None,
        };
        assert_eq!(read(json!({"id": ID, "isRead": true})), Some((true, true)));
        assert_eq!(read(json!({"@odata.type": "#microsoft.graph.message", "id": ID, "isRead": false})), Some((true, false)));
        // As Graph's own example of a delta writes it.
        assert_eq!(read(json!({"id": ID, "isRead": "true"})), Some((true, true)));
        // Nothing to take: passed over.
        for nothing in [json!({"id": ID}), json!({"id": ID, "isRead": 1}), json!({"id": ID, "isRead": "yes"}), json!({"isRead": true})] {
            assert!(entry_of(&nothing, "F").is_none(), "{nothing}");
        }
        // A whole message whose isRead is text is read as it says.
        let whole = json!({"id": ID, "receivedDateTime": "2027-01-15T08:00:00Z", "isRead": "true"});
        assert!(matches!(entry_of(&whole, "F"), Some(Entry::Message(m)) if m.server.read));
    }
}
